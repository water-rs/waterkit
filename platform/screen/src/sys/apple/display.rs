//! macOS display stream: an `SCStream` on one `SCDisplay`, delivering each
//! complete frame's `IOSurface`-backed `CVPixelBuffer` without a copy.

use core::time::Duration;
use std::sync::Mutex;

use block2::{DynBlock, RcBlock};
use dispatch2::{DispatchQueue, DispatchRetained};
use futures::channel::oneshot;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_media::{CMSampleBuffer, CMTime, CMTimeFlags};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetHeight, CVPixelBufferGetIOSurface,
    CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth, kCVPixelFormatType_32BGRA,
    kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, kCVPixelFormatType_ARGB2101010LEPacked,
};
use objc2_foundation::{NSError, NSObjectProtocol};
use objc2_io_surface::IOSurfaceRef;
use objc2_screen_capture_kit::{
    SCFrameStatus, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamDelegate,
    SCStreamFrameInfoStatus, SCStreamOutput, SCStreamOutputType,
};

use super::macos::{capture_error, display_filter, find_display};
use crate::Error;
use crate::display_stream::{DisplayStreamConfig, PixelBuffer, PixelFormat};

/// Surfaces the stream may have out at once: the most `SCStreamConfiguration`
/// accepts. A frame keeps its surface until the caller drops it.
const QUEUE_DEPTH: u8 = 8;

/// Undelivered frames the stream keeps. One surface fewer than the queue
/// depth, so capture always has a surface to draw the next frame into, and a
/// full channel replaces its oldest frame instead.
const FRAME_BACKLOG: usize = QUEUE_DEPTH as usize - 1;

const fn four_cc(format: PixelFormat) -> u32 {
    match format {
        PixelFormat::Bgra8 => kCVPixelFormatType_32BGRA,
        PixelFormat::Bgr10a2 => kCVPixelFormatType_ARGB2101010LEPacked,
        PixelFormat::Ycbcr420Video => kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
        PixelFormat::Ycbcr420Full => kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
    }
}

#[expect(
    non_upper_case_globals,
    reason = "the patterns are CoreVideo's own constant names"
)]
fn pixel_format(four_cc: u32) -> PixelFormat {
    match four_cc {
        kCVPixelFormatType_32BGRA => PixelFormat::Bgra8,
        kCVPixelFormatType_ARGB2101010LEPacked => PixelFormat::Bgr10a2,
        kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange => PixelFormat::Ycbcr420Video,
        kCVPixelFormatType_420YpCbCr8BiPlanarFullRange => PixelFormat::Ycbcr420Full,
        other => panic!(
            "ScreenCaptureKit delivered pixel format {other:#010x}, which no DisplayStreamConfig requests"
        ),
    }
}

/// A presentation timestamp on the host clock as nanoseconds since boot.
fn host_time(time: CMTime) -> Duration {
    assert!(
        time.flags.contains(CMTimeFlags::Valid) && time.value >= 0 && time.timescale > 0,
        "ScreenCaptureKit delivered a frame without a valid presentation time: {time:?}"
    );
    let nanos = i128::from(time.value) * 1_000_000_000 / i128::from(time.timescale);
    Duration::from_nanos(u64::try_from(nanos).expect("a host-clock time fits u64 nanoseconds"))
}

/// One complete frame's pixel buffer and the surface backing it.
#[derive(Debug)]
pub struct PixelBufferInner {
    buffer: CFRetained<CVPixelBuffer>,
    surface: CFRetained<IOSurfaceRef>,
    presentation_time: Duration,
}

// SAFETY: `CVPixelBuffer` and `IOSurface` are reference-counted CoreVideo /
// IOSurface objects whose retain, release and property reads are
// thread-safe; this wrapper never mutates them.
#[expect(
    clippy::non_send_fields_in_send_ty,
    reason = "objc2 marks CoreVideo and IOSurface objects !Send; the SAFETY note gives why these are"
)]
unsafe impl Send for PixelBufferInner {}
// SAFETY: as above; `&PixelBufferInner` only reads.
unsafe impl Sync for PixelBufferInner {}

impl PixelBufferInner {
    /// The frame a sample buffer carries, or `None` when the sample reports
    /// no new content (an idle, blank or suspended display).
    fn from_sample(sample: &CMSampleBuffer) -> Option<Self> {
        // SAFETY: `sample` is the live sample buffer ScreenCaptureKit handed
        // to the output callback.
        let attachments = unsafe { sample.sample_attachments_array(false) }
            .expect("ScreenCaptureKit attaches frame info to every sample");
        // SAFETY: the array holds one attachments dictionary per sample, keyed
        // by `CFString`s.
        let attachments: CFRetained<CFArray<CFDictionary<CFString, CFType>>> =
            unsafe { CFRetained::cast_unchecked(attachments) };
        let info = attachments
            .get(0)
            .expect("a screen sample carries one frame info dictionary");
        // SAFETY: `NSString` is toll-free bridged to `CFString`.
        let status_key: &CFString =
            unsafe { &*core::ptr::from_ref(SCStreamFrameInfoStatus).cast::<CFString>() };
        let status = info
            .get(status_key)
            .and_then(|value| value.downcast::<CFNumber>().ok())
            .and_then(|number| number.as_isize())
            .expect("a screen sample's frame info carries its SCFrameStatus");
        if SCFrameStatus(status) != SCFrameStatus::Complete {
            return None;
        }
        // SAFETY: `sample` is live.
        let buffer = unsafe { sample.image_buffer() }
            .expect("a complete ScreenCaptureKit frame carries an image buffer");
        let surface = CVPixelBufferGetIOSurface(Some(&buffer))
            .expect("ScreenCaptureKit frames are IOSurface-backed");
        // SAFETY: `sample` is live.
        let presentation_time = host_time(unsafe { sample.presentation_time_stamp() });
        Some(Self {
            buffer,
            surface,
            presentation_time,
        })
    }

    pub fn width(&self) -> u32 {
        u32::try_from(CVPixelBufferGetWidth(&self.buffer)).expect("a pixel buffer width fits u32")
    }

    pub fn height(&self) -> u32 {
        u32::try_from(CVPixelBufferGetHeight(&self.buffer)).expect("a pixel buffer height fits u32")
    }

    pub fn pixel_format(&self) -> PixelFormat {
        pixel_format(CVPixelBufferGetPixelFormatType(&self.buffer))
    }

    pub const fn presentation_time(&self) -> Duration {
        self.presentation_time
    }

    pub fn cv_pixel_buffer(&self) -> &CVPixelBuffer {
        &self.buffer
    }

    pub fn io_surface(&self) -> &IOSurfaceRef {
        &self.surface
    }
}

/// Where the stream's frames go once captured.
#[derive(Debug)]
pub struct StreamOutputIvars {
    frames: async_channel::Sender<PixelBuffer>,
}

define_class!(
    // SAFETY:
    // - The superclass NSObject does not have any subclassing requirements.
    // - `StreamOutput` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = AnyThread]
    #[name = "WaterkitDisplayStreamOutput"]
    #[ivars = StreamOutputIvars]
    #[derive(Debug)]
    pub struct StreamOutput;

    unsafe impl NSObjectProtocol for StreamOutput {}

    unsafe impl SCStreamOutput for StreamOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn did_output_sample_buffer(
            &self,
            _stream: &SCStream,
            sample_buffer: &CMSampleBuffer,
            r#type: SCStreamOutputType,
        ) {
            if r#type != SCStreamOutputType::Screen {
                return;
            }
            let Some(frame) = PixelBufferInner::from_sample(sample_buffer) else {
                return;
            };
            // Sending fails only once the channel is closed, which happens
            // when the `DisplayStream` drops: the frame has no reader left.
            let _ = self.ivars().frames.force_send(PixelBuffer::new(frame));
        }
    }

    unsafe impl SCStreamDelegate for StreamOutput {
        #[unsafe(method(stream:didStopWithError:))]
        fn did_stop_with_error(&self, _stream: &SCStream, error: &NSError) {
            tracing::error!(
                error = %error.localizedDescription(),
                "display stream stopped by ScreenCaptureKit"
            );
            self.ivars().frames.close();
        }
    }
);

impl StreamOutput {
    fn new(frames: async_channel::Sender<PixelBuffer>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(StreamOutputIvars { frames });
        // SAFETY: `this` is freshly allocated and `NSObject`'s `init` has no
        // additional requirements.
        unsafe { msg_send![super(this), init] }
    }
}

/// The `ScreenCaptureKit` objects one display stream owns.
#[derive(Debug)]
struct Capture {
    stream: Retained<SCStream>,
    /// Receives frames and stop errors; the stream holds it only weakly as
    /// its delegate.
    _output: Retained<StreamOutput>,
    /// The serial queue frames are delivered on, so they arrive in order.
    _queue: DispatchRetained<DispatchQueue>,
}

// SAFETY: `SCStream`'s capture control (`startCapture` / `stopCapture`) is
// asynchronous messaging to the capture service and is callable from any
// thread; `StreamOutput`'s ivars are a `Send + Sync` channel sender, and
// ScreenCaptureKit itself calls it from its own queues.
#[expect(
    clippy::non_send_fields_in_send_ty,
    reason = "objc2 marks ScreenCaptureKit objects !Send; the SAFETY note gives why these are"
)]
unsafe impl Send for Capture {}
// SAFETY: as above; `DisplayStreamInner` only touches the stream through
// `&mut self`.
unsafe impl Sync for Capture {}

impl Capture {
    fn open(
        content: &SCShareableContent,
        display_id: u32,
        config: &DisplayStreamConfig,
        frames: async_channel::Sender<PixelBuffer>,
    ) -> Result<Self, Error> {
        let display = find_display(content, display_id)?;
        let filter = display_filter(&display);
        // SAFETY: `SCStreamConfiguration` is a plain data object.
        let configuration = unsafe { SCStreamConfiguration::new() };
        let frame_rate =
            i32::try_from(config.frame_rate.get()).expect("a frame rate fits a CMTimeScale");
        // SAFETY: setters on a live configuration object; `CMTime::new` builds
        // a plain value.
        unsafe {
            configuration
                .setWidth(usize::try_from(config.width.get()).expect("a u32 width fits usize"));
            configuration
                .setHeight(usize::try_from(config.height.get()).expect("a u32 height fits usize"));
            configuration.setPixelFormat(four_cc(config.pixel_format));
            configuration.setMinimumFrameInterval(CMTime::new(1, frame_rate));
            configuration.setQueueDepth(isize::from(QUEUE_DEPTH));
        }
        let output = StreamOutput::new(frames);
        // SAFETY: `filter`, `configuration` and `output` are live; the stream
        // copies its configuration.
        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &configuration,
                Some(ProtocolObject::from_ref(&*output)),
            )
        };
        let queue = DispatchQueue::new("waterkit.screen.display-stream", None);
        // SAFETY: `output` conforms to `SCStreamOutput`; `queue` is a serial
        // queue that outlives the stream through `Capture`.
        unsafe {
            stream.addStreamOutput_type_sampleHandlerQueue_error(
                ProtocolObject::from_ref(&*output),
                SCStreamOutputType::Screen,
                Some(&queue),
            )
        }
        .map_err(|error| capture_error("SCStream addStreamOutput", &error))?;
        Ok(Self {
            stream,
            _output: output,
            _queue: queue,
        })
    }
}

/// The answering half of a completion handler's reply. A completion block is
/// `Fn` and may run on any queue, so the one-shot sender sits behind a lock
/// and the first call answers.
struct Reply<T>(Mutex<Option<oneshot::Sender<Result<T, Error>>>>);

impl<T> Reply<T> {
    fn send(&self, result: Result<T, Error>) {
        let sender = self.0.lock().unwrap().take();
        if let Some(sender) = sender {
            let _ = sender.send(result);
        }
    }
}

/// The awaiting half of a completion handler's reply.
struct Replied<T> {
    context: &'static str,
    receiver: oneshot::Receiver<Result<T, Error>>,
}

impl<T> Replied<T> {
    async fn wait(self) -> Result<T, Error> {
        self.receiver.await.map_err(|_| {
            Error::Platform(format!(
                "{} dropped its completion before replying",
                self.context
            ))
        })?
    }
}

/// A reply for the completion handler of the call named `context`.
fn reply<T>(context: &'static str) -> (Reply<T>, Replied<T>) {
    let (sender, receiver) = oneshot::channel();
    (
        Reply(Mutex::new(Some(sender))),
        Replied { context, receiver },
    )
}

/// `SCStream`'s start / stop capture calls.
type CaptureControl = unsafe fn(&SCStream, Option<&DynBlock<dyn Fn(*mut NSError)>>);

/// The macOS display stream behind [`crate::DisplayStream`].
#[derive(Debug)]
pub struct DisplayStreamInner {
    capture: Capture,
    frames: async_channel::Receiver<PixelBuffer>,
    running: bool,
}

impl DisplayStreamInner {
    pub async fn new(display_id: u32, config: DisplayStreamConfig) -> Result<Self, Error> {
        let (sender, frames) = async_channel::bounded(FRAME_BACKLOG);
        let opened = {
            let (reply, opened) = reply::<Capture>("SCShareableContent");
            let block = RcBlock::new(
                move |content: *mut SCShareableContent, error: *mut NSError| {
                    // SAFETY: a non-null `error` and `content` are valid for
                    // the callback's duration.
                    let result = match unsafe { (content.as_ref(), error.as_ref()) } {
                        (_, Some(error)) => Err(capture_error("SCShareableContent", error)),
                        (None, None) => Err(Error::Platform(
                            "SCShareableContent returned no content".into(),
                        )),
                        (Some(content), None) => {
                            Capture::open(content, display_id, &config, sender.clone())
                        }
                    };
                    reply.send(result);
                },
            );
            // SAFETY: `block` is a well-typed completion block; the API keeps
            // its own copy.
            unsafe {
                SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
                    false,
                    true,
                    &block,
                );
            }
            opened
        };
        let capture = opened.wait().await?;
        Ok(Self {
            capture,
            frames,
            running: false,
        })
    }

    pub fn frames(&self) -> async_channel::Receiver<PixelBuffer> {
        self.frames.clone()
    }

    /// Calls `control` on the stream and waits for its completion handler.
    async fn control(&self, context: &'static str, control: CaptureControl) -> Result<(), Error> {
        let replied = {
            let (reply, replied) = reply::<()>(context);
            let block = RcBlock::new(move |error: *mut NSError| {
                // SAFETY: a non-null `error` is valid for the callback's
                // duration.
                let result = unsafe { error.as_ref() }
                    .map_or(Ok(()), |error| Err(capture_error(context, error)));
                reply.send(result);
            });
            // SAFETY: `block` is a well-typed completion block; the stream
            // keeps its own copy.
            unsafe { control(&self.capture.stream, Some(&block)) };
            replied
        };
        replied.wait().await
    }

    pub async fn resume(&mut self) -> Result<(), Error> {
        if self.running {
            return Ok(());
        }
        self.control(
            "SCStream startCapture",
            SCStream::startCaptureWithCompletionHandler,
        )
        .await?;
        self.running = true;
        Ok(())
    }

    pub async fn pause(&mut self) -> Result<(), Error> {
        if !self.running {
            return Ok(());
        }
        self.control(
            "SCStream stopCapture",
            SCStream::stopCaptureWithCompletionHandler,
        )
        .await?;
        self.running = false;
        Ok(())
    }
}

impl Drop for DisplayStreamInner {
    fn drop(&mut self) {
        self.frames.close();
        if self.running {
            // SAFETY: stopping a running stream with no completion is valid.
            unsafe { self.capture.stream.stopCaptureWithCompletionHandler(None) };
        }
    }
}
