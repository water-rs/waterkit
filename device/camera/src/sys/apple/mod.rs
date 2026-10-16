//! Apple platform (iOS/macOS) camera implementation using `AVCaptureSession`.
//!
//! The capture output keeps the camera's native biplanar 4:2:0 format
//! (`420f` where offered, otherwise `420v`), and each frame's planes are the
//! capture buffer's `IOSurface` planes imported into wgpu without a copy (see
//! [`capture`]). The frame's orientation comes from the capture connection's
//! rotation and mirroring.
//!
//! The `AVCaptureSession` graph is not `Send` in `objc2-av-foundation` 0.3.x,
//! so it is created on, owned by and driven from one dedicated serial session
//! thread: `startRunning`/`stopRunning` block for seconds and may never run
//! on the main thread, nor be waited on synchronously from async code — every
//! request is queued to the session thread and its answer comes back through
//! a oneshot. The sample-buffer delegate runs on its own serial capture
//! `DispatchQueue` and only sends into channels; a dropped buffer makes the
//! delegate poll the wgpu device itself so leased textures are destroyed and
//! the capture buffers come back (#282, #319).

mod capture;

pub use capture::CapturedPixelBuffer;
use capture::RawFrame;

use crate::{
    AnalysisFrame, CameraCapabilities, CameraConfig, CameraControls, CameraError, CameraInfo,
    DynamicRangeProfile, ExposureControl, ExposureMode, FlashMode, FocusControl, FocusMode, Frame,
    Photo, RawPhoto, RawPhotoFormat, RawVideoFormat, Resolution, StabilizationMode,
    WhiteBalanceControl, WhiteBalanceMode,
};
use std::num::NonZeroU8;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
#[cfg(target_os = "ios")]
use objc2_av_foundation::{
    AVCaptureColorSpace, AVCaptureDeviceFormat, AVCaptureDeviceTypeBuiltInTelephotoCamera,
    AVCaptureDeviceTypeBuiltInUltraWideCamera, AVCaptureExposureMode, AVCaptureMultiCamSession,
    AVCaptureSessionPresetInputPriority, AVCaptureVideoStabilizationMode,
    AVCaptureWhiteBalanceTemperatureAndTintValues, AVVideoCodecTypeH264, AVVideoCodecTypeHEVC,
};
use objc2_av_foundation::{
    AVCaptureConnection, AVCaptureDevice, AVCaptureDeviceDiscoverySession, AVCaptureDeviceInput,
    AVCaptureDevicePosition, AVCaptureDeviceType, AVCaptureDeviceTypeBuiltInWideAngleCamera,
    AVCaptureFileOutput, AVCaptureFileOutputRecordingDelegate, AVCaptureFocusMode,
    AVCaptureMovieFileOutput, AVCaptureOutput, AVCapturePhoto, AVCapturePhotoCaptureDelegate,
    AVCapturePhotoOutput, AVCapturePhotoSettings, AVCaptureSession, AVCaptureSessionPreset,
    AVCaptureSessionPreset352x288, AVCaptureSessionPreset640x480, AVCaptureSessionPreset1280x720,
    AVCaptureSessionPreset1920x1080, AVCaptureSessionPreset3840x2160, AVCaptureSessionPresetHigh,
    AVCaptureTorchMode, AVCaptureVideoDataOutput, AVCaptureVideoDataOutputSampleBufferDelegate,
    AVCaptureWhiteBalanceMode, AVMediaTypeVideo, AVVideoCodecKey, AVVideoCodecTypeJPEG,
};
#[cfg(target_os = "macos")]
use objc2_av_foundation::{AVCaptureDeviceRotationCoordinator, AVCaptureDeviceTypeExternal};
use objc2_core_foundation::{CFRetained, CFString};
use objc2_core_media::{CMSampleBuffer, CMTimeFlags, CMTimeRoundingMethod};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRowOfPlane,
    CVPixelBufferGetHeightOfPlane, CVPixelBufferGetWidthOfPlane, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress, kCVReturnSuccess,
};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSNumber, NSString, NSURL};
use std::io::Write as _;

// `AVCaptureDevice`'s "keep the current value" sentinels for custom
// exposure; `objc2-av-foundation` 0.3.2 has no binding for them. The SDK
// renamed the exports to `AVCaptureISOCurrent` and
// `AVCaptureExposureDurationCurrent`.
#[cfg(target_os = "ios")]
unsafe extern "C" {
    static AVCaptureISOCurrent: f32;
    static AVCaptureExposureDurationCurrent: objc2_core_media::CMTime;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordingMode {
    Standard,
    Raw,
}

// ============================================================================
// Session thread
// ============================================================================
//
// The capture session's Objective-C objects are `!Send`, so they are created
// inside the session thread and never leave it; commands carry only `Send`
// data plus a reply channel. Dropping the command channel ends the thread,
// which stops the session before the objects die.

/// A unit of work the session thread runs; `&mut Session` hands it the
/// objects it was issued against, which live only on that thread.
type SessionWork = Box<dyn FnOnce(&mut Session) + Send>;

/// The serial session runner. The session graph is built inside `spawn`'s
/// initializer; `send` queues fire-and-forget work, while `ask` queues work
/// and awaits the oneshot the closure resolves.
struct SessionThread {
    commands: async_channel::Sender<SessionWork>,
    _thread: std::thread::JoinHandle<()>,
}

impl SessionThread {
    /// Spawns the thread and runs `init` inside it to build the session
    /// graph; the returned receiver answers the caller with `init`'s result.
    fn spawn<T: Send + 'static>(
        init: impl FnOnce(&mut Session) -> Result<T, CameraError> + Send + 'static,
    ) -> Result<
        (
            Self,
            futures::channel::oneshot::Receiver<Result<T, CameraError>>,
        ),
        CameraError,
    > {
        let (commands, inbox) = async_channel::unbounded::<SessionWork>();
        let (reply, answer) = futures::channel::oneshot::channel();
        let thread = std::thread::Builder::new()
            .name("waterkit.camera.session".into())
            .spawn(move || {
                let mut session = Session::new();
                let _ = reply.send(init(&mut session));
                while let Ok(work) = inbox.recv_blocking() {
                    work(&mut session);
                }
                session.stop();
            })
            .map_err(|error| CameraError::StartFailed(format!("session thread: {error}")))?;
        Ok((
            Self {
                commands,
                _thread: thread,
            },
            answer,
        ))
    }

    /// Queues `work` without waiting; the closure either runs
    /// fire-and-forget or resolves a oneshot captured inside it.
    fn send(&self, work: impl FnOnce(&mut Session) + Send + 'static) -> Result<(), CameraError> {
        self.commands
            .try_send(Box::new(work))
            .map_err(|_| CameraError::OpenFailed("capture session closed".into()))
    }

    /// Queues `work` and awaits its result. A failed send or a dead thread
    /// means the camera is closed.
    async fn ask<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut Session) -> T + Send + 'static,
    ) -> Result<T, CameraError> {
        let (reply, answer) = futures::channel::oneshot::channel();
        self.send(move |session| {
            let _ = reply.send(work(session));
        })?;
        answer
            .await
            .map_err(|_| CameraError::OpenFailed("capture session thread ended".into()))
    }
}

// ============================================================================
// Delegates
// ============================================================================

/// Everything the frame delegate needs across its callbacks is plain `Send`
/// data; the `AVCaptureConnection` it inspects is borrowed from the callback.
pub struct FrameDelegateIvars {
    sender: async_channel::Sender<Result<RawFrame, CameraError>>,
    /// Turns the sample buffers' presentation times into frame timestamps
    /// measured from the first captured frame.
    clock: crate::clock::StreamClock<Duration>,
    /// The device the camera was opened with: `didDrop` polls it so a
    /// starved stream's capture buffers come back.
    device: Arc<wgpu::Device>,
    /// The analysis stream's channel and its own clock, present only when
    /// the camera was opened with `CameraConfig::analysis`.
    analysis: Option<AnalysisOutput>,
    /// The upright angle last known, or `None` until orientation tracking
    /// first reports. Frames are not delivered before then.
    #[cfg(target_os = "ios")]
    display_angle: Arc<Mutex<Option<i64>>>,
    /// macOS 14+: the rotation coordinator reporting this device's capture
    /// angle; read per frame so a mid-stream rotation is seen.
    #[cfg(target_os = "macos")]
    rotation_coordinator: Option<Retained<AVCaptureDeviceRotationCoordinator>>,
    /// RAW video frame recording state shared with `CameraInner`.
    raw_video: Arc<Mutex<RawVideo>>,
    /// The output's biplanar 4:2:0 pixel format chosen at open.
    capture_pixel_format: u32,
}

/// The analysis stream's half of the delegate state.
struct AnalysisOutput {
    sender: async_channel::Sender<Result<RawFrame, CameraError>>,
    clock: crate::clock::StreamClock<Duration>,
}

define_class!(
    /// The video data output's sample-buffer delegate. Its ivars own the
    /// frame channels and the wgpu device, so a dropped sample buffer drives
    /// the reclaim directly — there is no C callback plumbing.
    #[unsafe(super(NSObject))]
    #[thread_kind = AnyThread]
    #[ivars = FrameDelegateIvars]
    struct FrameDelegate;

    unsafe impl NSObjectProtocol for FrameDelegate {}

    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for FrameDelegate {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        unsafe fn did_output(
            &self,
            _output: &AVCaptureOutput,
            sample_buffer: &CMSampleBuffer,
            connection: &AVCaptureConnection,
        ) {
            self.ivars().on_sample(sample_buffer, connection);
        }

        #[unsafe(method(captureOutput:didDropSampleBuffer:fromConnection:))]
        unsafe fn did_drop(
            &self,
            _output: &AVCaptureOutput,
            _sample_buffer: &CMSampleBuffer,
            _connection: &AVCaptureConnection,
        ) {
            self.ivars().on_drop();
        }
    }
);

impl FrameDelegate {
    fn new(ivars: FrameDelegateIvars) -> Retained<Self> {
        let this = Self::alloc().set_ivars(ivars);
        // SAFETY: init() on NSObject always succeeds.
        unsafe { msg_send![super(this), init] }
    }
}

/// The rotation angle each legacy `AVCaptureVideoOrientation` stands for, as
/// `videoRotationAngle` defines it (landscapeRight = 0, portrait = 90, …).
const fn legacy_rotation_degrees(orientation: isize) -> i64 {
    match orientation {
        // portrait = 90, portraitUpsideDown = 270, landscapeRight = 0,
        // landscapeLeft = 180.
        1 => 90,
        2 => 270,
        4 => 180,
        _ => 0,
    }
}

impl FrameDelegateIvars {
    fn on_sample(&self, sample_buffer: &CMSampleBuffer, connection: &AVCaptureConnection) {
        // SAFETY: `sample_buffer` is the live buffer AVFoundation delivered.
        let Some(image_buffer) = (unsafe { sample_buffer.image_buffer() }) else {
            return;
        };
        // SAFETY: a video data output's image buffer is a CVPixelBuffer; the
        // owned reference moves to `from_owned` unchanged.
        let pixel_buffer = unsafe {
            CapturedPixelBuffer::from_owned(
                CFRetained::into_raw(image_buffer).cast::<CVPixelBuffer>(),
            )
        };

        // The presentation time is the frame's capture-clock reading: the
        // capture session's synchronization clock, in host time.
        let pts = unsafe { sample_buffer.presentation_time_stamp() };
        assert!(
            pts.flags.0 & CMTimeFlags::ImpliedValueFlagsMask.0 == 0,
            "a capture buffer's presentation time is numeric"
        );
        let capture_time_ns = u64::try_from(
            unsafe {
                pts.convert_scale(1_000_000_000, CMTimeRoundingMethod::RoundHalfAwayFromZero)
            }
            .value,
        )
        .expect("a capture buffer's presentation time is non-negative");

        self.write_raw_video_frame(&pixel_buffer.0, capture_time_ns);
        if let Some((rotation_degrees, mirrored)) = self.frame_orientation(connection) {
            self.deliver(pixel_buffer, capture_time_ns, rotation_degrees, mirrored);
        }
    }

    /// One frame for both streams: the preview sender and the optional
    /// analysis sender, newest-wins on each.
    fn deliver(
        &self,
        pixel_buffer: CapturedPixelBuffer,
        capture_time_ns: u64,
        rotation_degrees: u32,
        mirrored: bool,
    ) {
        if let Some(analysis) = &self.analysis {
            let _ = analysis.sender.force_send(Ok(RawFrame {
                pixel_buffer: pixel_buffer.clone(),
                timestamp: analysis
                    .clock
                    .timestamp(Duration::from_nanos(capture_time_ns)),
                rotation_degrees,
                mirrored,
            }));
        }
        // Newest wins: a frame the consumer has not taken yet is displaced,
        // and dropping it returns its buffer to the capture pool at once.
        let _ = self.sender.force_send(Ok(RawFrame {
            pixel_buffer,
            timestamp: self.clock.timestamp(Duration::from_nanos(capture_time_ns)),
            rotation_degrees,
            mirrored,
        }));
    }

    /// A dropped frame means every capture buffer is checked out; they return
    /// only through wgpu's device maintenance, which runs inside
    /// `queue.submit` and `device.poll`. A consumer awaiting the stream
    /// submits nothing, so the camera runs one non-blocking poll itself per
    /// drop. A poll failure is a reader failure: it is each stream's last
    /// item.
    fn on_drop(&self) {
        if let Err(error) = self.device.poll(wgpu::PollType::Poll) {
            let error = CameraError::GpuError(format!("device poll: {error}"));
            if let Some(analysis) = &self.analysis {
                let _ = analysis.sender.force_send(Err(error.clone()));
                analysis.sender.close();
            }
            let _ = self.sender.force_send(Err(error));
            self.sender.close();
        }
    }

    /// The clockwise rotation, in degrees, that turns this connection's
    /// buffers upright on the display once any mirroring is undone, and
    /// whether the connection mirrors; `None` while the display's
    /// orientation is unknown.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a rotation angle is 0..360"
    )]
    fn frame_orientation(&self, connection: &AVCaptureConnection) -> Option<(u32, bool)> {
        // The capture rotation matching the display.
        #[cfg(target_os = "ios")]
        let display = {
            // The interface may turn at any frame; the next frame sees the
            // change.
            refresh_display_angle(&self.display_angle);
            *self.display_angle.lock().expect("display angle lock")
        };
        #[cfg(target_os = "macos")]
        let display = Some(self.rotation_coordinator.as_ref().map_or(0, |coordinator| {
            // SAFETY: reading the horizon-level angle has no side effects.
            unsafe { coordinator.videoRotationAngleForHorizonLevelCapture() }.round() as i64
        }));
        let display = display?;
        // SAFETY: `videoRotationAngle` exists since iOS 17/macOS 14; on
        // older systems `videoOrientation` carries the same information.
        let applied = unsafe {
            if objc2::available!(ios = 17.0, macos = 14.0, ..) {
                let angle: f64 = msg_send![connection, videoRotationAngle];
                angle.round() as i64
            } else {
                let orientation: isize = msg_send![connection, videoOrientation];
                legacy_rotation_degrees(orientation)
            }
        };
        // SAFETY: reading `isVideoMirrored` is side-effect free.
        let mirrored = unsafe { connection.isVideoMirrored() };
        let degrees = u32::try_from((display - applied).rem_euclid(360))
            .expect("degrees mod 360 are non-negative");
        Some((degrees, mirrored))
    }
}

/// iOS: the capture rotation matching the foreground scene's interface
/// orientation, read on the main thread where `UIKit` allows it, keeping the
/// last known angle while no scene is in the foreground.
#[cfg(target_os = "ios")]
fn refresh_display_angle(display_angle: &Arc<Mutex<Option<i64>>>) {
    let display_angle = Arc::clone(display_angle);
    DispatchQueue::main().exec_async(move || {
        if let Some(angle) = interface_rotation_angle() {
            *display_angle.lock().expect("display angle lock") = Some(angle);
        }
    });
}

/// iOS: the rotation of the app's foreground scene, `None` while no scene is
/// foregrounded (main thread only).
#[cfg(target_os = "ios")]
fn interface_rotation_angle() -> Option<i64> {
    use objc2_ui_kit::{UIApplication, UISceneActivationState, UIWindowScene};
    let mtm = objc2::MainThreadMarker::new()
        .expect("a main-queue `exec_async` closure runs on the main thread");
    let application = UIApplication::sharedApplication(mtm);
    let scenes = application.connectedScenes();
    for scene in &scenes {
        if scene.activationState() != UISceneActivationState::ForegroundActive {
            continue;
        }
        if !scene.isKindOfClass(objc2::class!(UIWindowScene)) {
            continue;
        }
        // SAFETY: `isKindOfClass` proved the scene is a UIWindowScene.
        let scene = unsafe { &*(core::ptr::from_ref(&*scene).cast::<UIWindowScene>()) };
        #[expect(
            deprecated,
            reason = "no non-deprecated replacement exposes the scene orientation"
        )]
        let orientation = scene.interfaceOrientation();
        return match orientation.0 {
            // landscapeRight = 0, portrait = 90, landscapeLeft = 180,
            // portraitUpsideDown = 270, unknown stays unreported.
            1 => Some(90),
            3 => Some(0),
            4 => Some(180),
            2 => Some(270),
            _ => None,
        };
    }
    None
}

pub struct PhotoDelegateIvars {
    /// The photo output the capture was issued on: the completion owns it,
    /// so it lives until the framework answers.
    _output: Retained<AVCapturePhotoOutput>,
    answer: Mutex<Option<PhotoAnswer>>,
}

/// The oneshot that resolves one photo capture.
type PhotoAnswer = futures::channel::oneshot::Sender<Result<Vec<u8>, CameraError>>;

define_class!(
    /// Resolves one photo capture: `fileDataRepresentation` bytes on
    /// success, or the error's description.
    #[unsafe(super(NSObject))]
    #[thread_kind = AnyThread]
    #[ivars = PhotoDelegateIvars]
    struct PhotoDelegate;

    unsafe impl NSObjectProtocol for PhotoDelegate {}

    unsafe impl AVCapturePhotoCaptureDelegate for PhotoDelegate {
        #[unsafe(method(photoOutput:didFinishProcessingPhoto:error:))]
        unsafe fn did_finish(
            &self,
            _output: &AVCapturePhotoOutput,
            photo: Option<&AVCapturePhoto>,
            error: Option<&NSError>,
        ) {
            let result = error.map_or_else(
                || {
                    photo.map_or_else(
                        || Err(CameraError::CaptureFailed("no photo".into())),
                        |photo| {
                            unsafe { photo.fileDataRepresentation() }
                                .map(|data| data.to_vec())
                                .ok_or_else(|| {
                                    CameraError::CaptureFailed("empty photo data".into())
                                })
                        },
                    )
                },
                |error| {
                    Err(CameraError::CaptureFailed(
                        error.localizedDescription().to_string(),
                    ))
                },
            );
            let mut answer = self.ivars().answer.lock().expect("photo answer lock");
            if let Some(answer) = answer.take() {
                let _ = answer.send(result);
            }
        }
    }
);

impl PhotoDelegate {
    fn new(
        output: &Retained<AVCapturePhotoOutput>,
    ) -> (
        Retained<Self>,
        futures::channel::oneshot::Receiver<Result<Vec<u8>, CameraError>>,
    ) {
        let (answer, receiver) = futures::channel::oneshot::channel();
        let this = Self::alloc().set_ivars(PhotoDelegateIvars {
            _output: output.clone(),
            answer: Mutex::new(Some(answer)),
        });
        // SAFETY: init() on NSObject always succeeds.
        (unsafe { msg_send![super(this), init] }, receiver)
    }
}

pub struct RecordingDelegateIvars {
    /// The standard recording's start instant, shared with `CameraInner`;
    /// finishing clears it.
    start: Arc<Mutex<Option<Instant>>>,
}

define_class!(
    /// Marks a movie recording finished when the file output reports it.
    /// `AVCaptureFileOutput` does not retain the recording delegate, so the
    /// session keeps it alive for the session's lifetime.
    #[unsafe(super(NSObject))]
    #[thread_kind = AnyThread]
    #[ivars = RecordingDelegateIvars]
    struct RecordingDelegate;

    unsafe impl NSObjectProtocol for RecordingDelegate {}

    unsafe impl AVCaptureFileOutputRecordingDelegate for RecordingDelegate {
        #[unsafe(method(captureOutput:didFinishRecordingToOutputFileAtURL:fromConnections:error:))]
        unsafe fn did_finish(
            &self,
            _output: &AVCaptureFileOutput,
            _url: &NSURL,
            _connections: &NSArray<AVCaptureConnection>,
            _error: Option<&NSError>,
        ) {
            *self.ivars().start.lock().expect("recording start lock") = None;
        }
    }
);

impl RecordingDelegate {
    fn new(start: Arc<Mutex<Option<Instant>>>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(RecordingDelegateIvars { start });
        // SAFETY: init() on NSObject always succeeds.
        unsafe { msg_send![super(this), init] }
    }
}

// ============================================================================
// RAW video recording (WKRV frame stream)
// ============================================================================

/// The raw frame file and its write state; written by the frame delegate and
/// shared with `CameraInner` so `stop_raw_recording` and `Drop` end it.
#[derive(Default)]
struct RawVideo {
    file: Option<std::fs::File>,
    /// The YCbCr matrix the stream opened with; a mid-recording change ends
    /// it, because the header can only name one.
    initial_matrix: Option<u8>,
    start: Option<Instant>,
}

fn append_u32_le(value: u32, data: &mut Vec<u8>) {
    data.extend_from_slice(&value.to_le_bytes());
}

fn append_u64_le(value: u64, data: &mut Vec<u8>) {
    data.extend_from_slice(&value.to_le_bytes());
}

/// The H.273 code for the buffer's `kCVImageBufferYCbCrMatrixKey` attachment.
#[expect(
    clippy::match_wildcard_for_single_variants,
    reason = "every matrix without an H.273 code fails the same way"
)]
fn raw_video_matrix_code(pixel_buffer: &CVPixelBuffer) -> Result<u8, String> {
    match capture::ycbcr_matrix(pixel_buffer) {
        waterkit_video_core::MatrixCoefficients::Bt601 => Ok(6),
        waterkit_video_core::MatrixCoefficients::Bt709 => Ok(1),
        waterkit_video_core::MatrixCoefficients::Bt2020NonConstantLuminance => Ok(9),
        other => Err(format!("H.273 has no code for {other:?}")),
    }
}

impl FrameDelegateIvars {
    /// Appends one frame as its luma rows followed by its interleaved chroma
    /// rows, without row padding.
    fn write_raw_video_frame(&self, pixel_buffer: &CVPixelBuffer, timestamp_ns: u64) {
        {
            let lock = self.raw_video.lock().expect("raw video lock");
            if lock.file.is_none() {
                return;
            }
        }
        let matrix_code = match raw_video_matrix_code(pixel_buffer) {
            Ok(code) => code,
            Err(value) => {
                self.fail_raw_video(&format!(
                    "missing or unsupported kCVImageBufferYCbCrMatrixKey value: {value}"
                ));
                return;
            }
        };
        let range_code = match self.capture_pixel_format {
            objc2_core_video::kCVPixelFormatType_420YpCbCr8BiPlanarFullRange => 1,
            objc2_core_video::kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange => 0,
            _ => {
                self.fail_raw_video("unsupported capture pixel format for WKRV NV12 range");
                return;
            }
        };

        // SAFETY: locking the delivered buffer's base address for reading is
        // the documented way to reach the planes.
        if unsafe { CVPixelBufferLockBaseAddress(pixel_buffer, CVPixelBufferLockFlags::ReadOnly) }
            != kCVReturnSuccess
        {
            return;
        }
        let result = (|| {
            let mut payload = Vec::new();
            // Luma holds one byte per pixel, chroma one Cb/Cr byte pair per
            // 2x2 block.
            for plane in 0..2usize {
                let rows = CVPixelBufferGetHeightOfPlane(pixel_buffer, plane);
                let row_bytes = CVPixelBufferGetWidthOfPlane(pixel_buffer, plane)
                    * if plane == 0 { 1 } else { 2 };
                let Some(base) = core::ptr::NonNull::new(
                    CVPixelBufferGetBaseAddressOfPlane(pixel_buffer, plane).cast::<u8>(),
                ) else {
                    return Err(format!("missing base address for raw video plane {plane}"));
                };
                let stride = CVPixelBufferGetBytesPerRowOfPlane(pixel_buffer, plane);
                for row in 0..rows {
                    // SAFETY: rows * stride stays within the locked plane.
                    let start = unsafe { base.as_ptr().add(row * stride) };
                    payload
                        .extend_from_slice(unsafe { std::slice::from_raw_parts(start, row_bytes) });
                }
            }
            Ok::<_, String>(payload)
        })();
        unsafe { CVPixelBufferUnlockBaseAddress(pixel_buffer, CVPixelBufferLockFlags::ReadOnly) };
        let payload = match result {
            Ok(payload) => payload,
            Err(message) => {
                self.fail_raw_video(&message);
                return;
            }
        };

        let mut lock = self.raw_video.lock().expect("raw video lock");
        let first_frame = lock.initial_matrix.is_none();
        if let Some(first) = lock.initial_matrix {
            if first != matrix_code {
                drop(lock);
                self.fail_raw_video(&format!(
                    "raw video YCbCr matrix changed from H.273 code {first} to {matrix_code}"
                ));
                return;
            }
        } else {
            lock.initial_matrix = Some(matrix_code);
        }
        let Some(file) = lock.file.as_mut() else {
            return;
        };
        if first_frame {
            // The first frame carries the WKRV header.
            let mut header = b"WKRV".to_vec();
            header.extend_from_slice(&[2, 3, matrix_code, range_code]);
            let (width, height) = (
                objc2_core_video::CVPixelBufferGetWidth(pixel_buffer),
                objc2_core_video::CVPixelBufferGetHeight(pixel_buffer),
            );
            append_u32_le(
                u32::try_from(width).expect("a capture dimension fits u32"),
                &mut header,
            );
            append_u32_le(
                u32::try_from(height).expect("a capture dimension fits u32"),
                &mut header,
            );
            append_u32_le(0, &mut header); // fps unknown from this layer
            let _ = file.write_all(&header);
        }
        let mut frame_header = Vec::with_capacity(12);
        append_u64_le(timestamp_ns, &mut frame_header);
        append_u32_le(
            u32::try_from(payload.len()).expect("a frame payload fits u32"),
            &mut frame_header,
        );
        let _ = file.write_all(&frame_header);
        let _ = file.write_all(&payload);
        drop(lock);
    }

    /// A failed raw recording logs and ends: the file closes when its handle
    /// drops.
    fn fail_raw_video(&self, message: &str) {
        tracing::error!("WaterkitCamera RAW video: {message}");
        let mut lock = self.raw_video.lock().expect("raw video lock");
        drop(lock.file.take());
        lock.initial_matrix = None;
        lock.start = None;
    }
}

// ============================================================================
// The session graph (lives entirely on the session thread)
// ============================================================================

/// Every `Retained` in the capture graph; created inside the session thread
/// and torn down when it ends.
struct Session {
    capture: Option<Retained<AVCaptureSession>>,
    device: Option<Retained<AVCaptureDevice>>,
    video_output: Option<Retained<AVCaptureVideoDataOutput>>,
    photo_output: Option<Retained<AVCapturePhotoOutput>>,
    movie_output: Option<Retained<AVCaptureMovieFileOutput>>,
    /// The sample-buffer delegate's serial capture queue; kept so the output
    /// never outlives the queue it calls on.
    capture_queue: Option<DispatchRetained<DispatchQueue>>,
    /// The sample-buffer and recording delegates; `AVFoundation` holds them
    /// weakly, so the session keeps them for its lifetime.
    frame_delegate: Option<Retained<FrameDelegate>>,
    recording_delegate: Option<Retained<RecordingDelegate>>,
    /// iOS: the closest SDR / Dolby Vision formats to the active one, for
    /// the dynamic-range control.
    #[cfg(target_os = "ios")]
    sdr_format: Option<Retained<AVCaptureDeviceFormat>>,
    #[cfg(target_os = "ios")]
    dolby_vision_format: Option<Retained<AVCaptureDeviceFormat>>,
    /// The standard recording's start instant, shared with `CameraInner`.
    recording_start: Arc<Mutex<Option<Instant>>>,
    raw_video: Arc<Mutex<RawVideo>>,
}

/// `open`'s payload back to the caller: the values `CameraInner` keeps.
struct OpenedSession {
    capabilities: CameraCapabilities,
    resolution: Resolution,
    /// The standard recording's start instant, written by the session thread
    /// and the recording delegate, read by `recording_duration` on the caller.
    recording_start: Arc<Mutex<Option<Instant>>>,
    /// The raw recording's write state, for `raw_recording_duration`.
    raw_video: Arc<Mutex<RawVideo>>,
}

fn device_types() -> Retained<NSArray<AVCaptureDeviceType>> {
    #[cfg(target_os = "ios")]
    // SAFETY: the device-type statics are AVFoundation's own constants.
    let types = unsafe {
        [
            AVCaptureDeviceTypeBuiltInWideAngleCamera,
            AVCaptureDeviceTypeBuiltInTelephotoCamera,
            AVCaptureDeviceTypeBuiltInUltraWideCamera,
        ]
    };
    #[cfg(not(target_os = "ios"))]
    // SAFETY: the device-type statics are AVFoundation's own constants.
    let types = unsafe {
        [
            AVCaptureDeviceTypeBuiltInWideAngleCamera,
            AVCaptureDeviceTypeExternal,
        ]
    };
    NSArray::from_slice(&types)
}

/// macOS 14+: this device's rotation coordinator. `initWithDevice:` is
/// gated behind `objc2-av-foundation`'s `objc2-quartz-core` feature (the
/// preview-layer parameter is a `CALayer`), so it is sent by hand with a nil
/// layer.
#[cfg(target_os = "macos")]
fn make_rotation_coordinator(
    device: &AVCaptureDevice,
) -> Option<Retained<AVCaptureDeviceRotationCoordinator>> {
    if !objc2::available!(macos = 14.0, ..) {
        return None;
    }
    // SAFETY: `initWithDevice:previewLayer:` accepts a nil preview layer;
    // creating the coordinator starts no capture.
    unsafe {
        msg_send![
            AVCaptureDeviceRotationCoordinator::alloc(),
            initWithDevice: device,
            previewLayer: core::ptr::null::<AnyObject>(),
        ]
    }
}

fn discover_devices() -> Retained<NSArray<AVCaptureDevice>> {
    // SAFETY: `AVMediaTypeVideo` is AVFoundation's own constant.
    let video = unsafe { AVMediaTypeVideo }.expect("AVMediaTypeVideo is exported");
    // SAFETY: a DiscoverySession with fixed device types has no side
    // effects beyond enumerating hardware.
    unsafe {
        AVCaptureDeviceDiscoverySession::discoverySessionWithDeviceTypes_mediaType_position(
            &device_types(),
            Some(video),
            AVCaptureDevicePosition::Unspecified,
        )
        .devices()
    }
}

/// A pixel format's four-character code, such as `420f`.
fn four_char_code(format: u32) -> String {
    let bytes = [24, 16, 8, 0].map(|shift| ((format >> shift) & 0xff) as u8);
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The capture output's biplanar 4:2:0 pixel format: `420f` when the device
/// offers it, otherwise `420v`.
fn choose_capture_format(output: &AVCaptureVideoDataOutput) -> Option<u32> {
    let offered = unsafe { output.availableVideoCVPixelFormatTypes() };
    for format in &offered {
        // SAFETY: pixel format types are NSNumbers.
        let value: u32 = unsafe { msg_send![&*format, unsignedIntValue] };
        if value == objc2_core_video::kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
            || value == objc2_core_video::kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
        {
            return Some(value);
        }
    }
    None
}

impl Session {
    fn new() -> Self {
        Self {
            capture: None,
            device: None,
            video_output: None,
            photo_output: None,
            movie_output: None,
            capture_queue: None,
            frame_delegate: None,
            recording_delegate: None,
            #[cfg(target_os = "ios")]
            sdr_format: None,
            #[cfg(target_os = "ios")]
            dolby_vision_format: None,
            recording_start: Arc::new(Mutex::new(None)),
            raw_video: Arc::new(Mutex::new(RawVideo::default())),
        }
    }

    /// Builds the capture graph and starts it. Runs on the session thread;
    /// the blocking `startRunning` must happen here and never on main.
    #[expect(
        clippy::too_many_lines,
        reason = "the graph is built in one pass; the delegates are the natural split"
    )]
    fn open(
        &mut self,
        camera_id: &str,
        frame_sender: async_channel::Sender<Result<RawFrame, CameraError>>,
        wgpu_device: &Arc<wgpu::Device>,
        analysis: Option<AnalysisOutput>,
    ) -> Result<OpenedSession, CameraError> {
        let devices = discover_devices();
        let mut found = None;
        for index in 0..devices.count() {
            let candidate = devices.objectAtIndex(index);
            if unsafe { candidate.uniqueID().to_string() } == camera_id {
                found = Some(candidate);
                break;
            }
        }
        let Some(device) = found else {
            return Err(CameraError::NotFound(camera_id.into()));
        };
        let name = unsafe { device.localizedName() }.to_string();

        let session = unsafe { AVCaptureSession::new() };
        unsafe { session.setSessionPreset(AVCaptureSessionPresetHigh) };
        #[cfg(target_os = "ios")]
        unsafe {
            session.setAutomaticallyConfiguresCaptureDeviceForWideColor(false);
        }

        // SAFETY: deviceInputWithDevice:error: reports permission and busy
        // failures through its error.
        let input = unsafe { AVCaptureDeviceInput::deviceInputWithDevice_error(&device) }.map_err(
            |error| {
                CameraError::OpenFailed(format!(
                    "{name} cannot be opened: {}",
                    error.localizedDescription()
                ))
            },
        )?;
        if unsafe { session.canAddInput(&input) } {
            unsafe { session.addInput(&input) };
        } else {
            return Err(CameraError::OpenFailed(format!(
                "the capture session cannot take {name} as its input"
            )));
        }

        let output = unsafe { AVCaptureVideoDataOutput::new() };
        // Keep the camera's native biplanar 4:2:0 layout so frames reach the
        // GPU as their IOSurface planes, without a conversion: full range
        // when the device offers it, video range otherwise.
        let Some(capture_pixel_format) = choose_capture_format(&output) else {
            let offered = unsafe { output.availableVideoCVPixelFormatTypes() }
                .iter()
                .map(|format| {
                    // SAFETY: pixel format types are NSNumbers.
                    let value: u32 = unsafe { msg_send![&*format, unsignedIntValue] };
                    four_char_code(value)
                })
                .collect::<Vec<_>>()
                .join(", ");
            return Err(CameraError::OpenFailed(format!(
                "{name} offers neither 420f nor 420v frames, which the camera needs to hand \
                 them to the GPU as they are; it offers [{offered}]"
            )));
        };
        let pixel_format_key = unsafe { objc2_core_video::kCVPixelBufferPixelFormatTypeKey };
        // SAFETY: the pixel-format key is a toll-free CFString.
        let pixel_format_key =
            unsafe { &*std::ptr::from_ref::<CFString>(pixel_format_key).cast::<NSString>() };
        let video_settings = NSDictionary::<NSString, AnyObject>::from_slices(
            &[pixel_format_key],
            &[&*NSNumber::new_u32(capture_pixel_format)],
        );
        unsafe { output.setVideoSettings(Some(&video_settings)) };

        let capture_queue = DispatchQueue::new("waterkit.camera.frame", None);
        let delegate = FrameDelegate::new(FrameDelegateIvars {
            sender: frame_sender,
            clock: crate::clock::StreamClock::new(),
            device: Arc::clone(wgpu_device),
            analysis,
            #[cfg(target_os = "ios")]
            display_angle: Arc::new(Mutex::new(None)),
            #[cfg(target_os = "macos")]
            rotation_coordinator: make_rotation_coordinator(&device),
            raw_video: Arc::clone(&self.raw_video),
            capture_pixel_format,
        });
        unsafe {
            output.setSampleBufferDelegate_queue(
                Some(ProtocolObject::from_ref(&*delegate)),
                Some(&capture_queue),
            );
        };
        unsafe { output.setAlwaysDiscardsLateVideoFrames(true) };

        if unsafe { session.canAddOutput(&output) } {
            unsafe { session.addOutput(&output) };
        } else {
            return Err(CameraError::OpenFailed(format!(
                "the capture session cannot add a video data output for {name}"
            )));
        }

        let photo_output = unsafe { AVCapturePhotoOutput::new() };
        let photo_output = if unsafe { session.canAddOutput(&photo_output) } {
            unsafe { session.addOutput(&photo_output) };
            #[cfg(target_os = "ios")]
            #[expect(
                deprecated,
                reason = "kept for parity with the previous implementation"
            )]
            unsafe {
                photo_output.setHighResolutionCaptureEnabled(true);
            }
            Some(photo_output)
        } else {
            None
        };

        let movie_output = unsafe { AVCaptureMovieFileOutput::new() };
        let movie_output = if unsafe { session.canAddOutput(&movie_output) } {
            unsafe { session.addOutput(&movie_output) };
            Some(movie_output)
        } else {
            None
        };

        let capabilities = query_capabilities(
            &device,
            movie_output.as_deref(),
            photo_output.as_deref(),
            self,
        );

        self.capture = Some(session);
        self.device = Some(device);
        self.video_output = Some(output);
        self.photo_output = photo_output;
        self.movie_output = movie_output;
        self.capture_queue = Some(capture_queue);
        self.frame_delegate = Some(delegate);
        self.recording_delegate = Some(RecordingDelegate::new(Arc::clone(&self.recording_start)));

        Ok(OpenedSession {
            capabilities,
            resolution: Resolution::HD,
            recording_start: Arc::clone(&self.recording_start),
            raw_video: Arc::clone(&self.raw_video),
        })
    }

    /// `startRunning` blocks for seconds: it runs here on the session thread,
    /// after `open` built the graph.
    fn start(&self, width: u32, height: u32) -> Result<Resolution, CameraError> {
        let session = self
            .capture
            .as_ref()
            .ok_or_else(|| CameraError::StartFailed("capture session closed".into()))?;
        // Nearest preset the session can take.
        // SAFETY: the preset statics are AVFoundation's own constants.
        let presets: [(&AVCaptureSessionPreset, i64, i64); 5] = unsafe {
            [
                (AVCaptureSessionPreset3840x2160, 3840, 2160),
                (AVCaptureSessionPreset1920x1080, 1920, 1080),
                (AVCaptureSessionPreset1280x720, 1280, 720),
                (AVCaptureSessionPreset640x480, 640, 480),
                (AVCaptureSessionPreset352x288, 352, 288),
            ]
        };
        // SAFETY: the preset static is AVFoundation's own constant.
        let mut best = unsafe { AVCaptureSessionPresetHigh };
        let mut best_diff = i64::MAX;
        for (preset, w, h) in presets {
            let diff = (i64::from(width) - w).abs() + (i64::from(height) - h).abs();
            if diff < best_diff && unsafe { session.canSetSessionPreset(preset) } {
                best_diff = diff;
                best = preset;
            }
        }
        unsafe {
            session.beginConfiguration();
            session.setSessionPreset(best);
            session.commitConfiguration();
            if !session.isRunning() {
                session.startRunning();
            }
        }
        let preset = unsafe { session.sessionPreset() };
        let resolution = preset_resolution(&preset);
        Ok(resolution)
    }

    fn stop(&mut self) {
        if let Some(movie) = &self.movie_output
            && unsafe { movie.isRecording() }
        {
            unsafe { movie.stopRecording() };
        }
        *self.raw_video.lock().expect("raw video lock") = RawVideo::default();
        if let Some(session) = &self.capture
            && unsafe { session.isRunning() }
        {
            unsafe { session.stopRunning() };
        }
        *self.recording_start.lock().expect("recording start lock") = None;
        self.capture = None;
        self.device = None;
        self.video_output = None;
        self.photo_output = None;
        self.movie_output = None;
        self.capture_queue = None;
        self.frame_delegate = None;
        self.recording_delegate = None;
    }

    /// The camera device, or `OpenFailed` when closed.
    fn device(&self) -> Result<&Retained<AVCaptureDevice>, CameraError> {
        self.device
            .as_ref()
            .ok_or_else(|| CameraError::OpenFailed("capture session closed".into()))
    }

    /// Lock the device for configuration; `body` applies the control and the
    /// device is unlocked after it, matching the previous `lockForConfiguration`
    /// pair. A lock failure maps to `OpenFailed`.
    fn configure_device(
        &self,
        body: impl FnOnce(&Retained<AVCaptureDevice>) -> Result<(), CameraError>,
    ) -> Result<(), CameraError> {
        let device = self.device()?.clone();
        unsafe { device.lockForConfiguration() }
            .map_err(|_| CameraError::OpenFailed("device lock failed".into()))?;
        let result = body(&device);
        unsafe { device.unlockForConfiguration() };
        result
    }

    #[cfg_attr(
        target_os = "macos",
        expect(clippy::unused_self, reason = "the control exists only on iOS")
    )]
    fn set_exposure_mode(&self, mode: ExposureMode) -> Result<(), CameraError> {
        #[cfg(target_os = "ios")]
        {
            let av_mode = match mode {
                ExposureMode::Auto => AVCaptureExposureMode::ContinuousAutoExposure,
                ExposureMode::Manual => AVCaptureExposureMode::Custom,
                ExposureMode::Locked => AVCaptureExposureMode::Locked,
            };
            if !unsafe { self.device()?.isExposureModeSupported(av_mode) } {
                return Err(CameraError::ControlUnsupported("exposure_mode".into()));
            }
            self.configure_device(|device| {
                unsafe { device.setExposureMode(av_mode) };
                Ok(())
            })
        }
        #[cfg(not(target_os = "ios"))]
        {
            let _ = mode;
            Err(CameraError::ControlUnsupported("exposure_mode".into()))
        }
    }

    #[cfg_attr(
        target_os = "macos",
        expect(clippy::unused_self, reason = "the control exists only on iOS")
    )]
    fn set_iso(&self, iso: f32) -> Result<(), CameraError> {
        #[cfg(target_os = "ios")]
        {
            let format = unsafe { self.device()?.activeFormat() };
            let clamped = iso.clamp(unsafe { format.minISO() }, unsafe { format.maxISO() });
            self.configure_device(|device| {
                unsafe {
                    device.setExposureModeCustomWithDuration_ISO_completionHandler(
                        AVCaptureExposureDurationCurrent,
                        clamped,
                        None,
                    );
                };
                Ok(())
            })
        }
        #[cfg(not(target_os = "ios"))]
        {
            let _ = iso;
            Err(CameraError::ControlUnsupported("iso".into()))
        }
    }

    #[cfg_attr(
        target_os = "macos",
        expect(clippy::unused_self, reason = "the control exists only on iOS")
    )]
    #[cfg_attr(
        target_os = "ios",
        expect(
            clippy::cast_possible_truncation,
            reason = "a clamped exposure in nanoseconds fits i64"
        )
    )]
    fn set_exposure_duration(&self, duration: Duration) -> Result<(), CameraError> {
        #[cfg(target_os = "ios")]
        {
            let format = unsafe { self.device()?.activeFormat() };
            let (min, max) = unsafe {
                (
                    format.minExposureDuration().seconds(),
                    format.maxExposureDuration().seconds(),
                )
            };
            let clamped_secs = duration.as_secs_f64().clamp(min, max);
            // SAFETY: constructing a CMTime is valid for any arguments.
            let clamped = unsafe {
                objc2_core_media::CMTime::new((clamped_secs * 1e9) as i64, 1_000_000_000)
            };
            self.configure_device(|device| {
                unsafe {
                    device.setExposureModeCustomWithDuration_ISO_completionHandler(
                        clamped,
                        AVCaptureISOCurrent,
                        None,
                    );
                };
                Ok(())
            })
        }
        #[cfg(not(target_os = "ios"))]
        {
            let _ = duration;
            Err(CameraError::ControlUnsupported("exposure_duration".into()))
        }
    }

    #[cfg_attr(
        target_os = "macos",
        expect(clippy::unused_self, reason = "the control exists only on iOS")
    )]
    fn set_exposure_compensation(&self, ev: f32) -> Result<(), CameraError> {
        #[cfg(target_os = "ios")]
        {
            let (min, max) = unsafe {
                (
                    self.device()?.minExposureTargetBias(),
                    self.device()?.maxExposureTargetBias(),
                )
            };
            self.configure_device(|device| {
                unsafe { device.setExposureTargetBias_completionHandler(ev.clamp(min, max), None) };
                Ok(())
            })
        }
        #[cfg(not(target_os = "ios"))]
        {
            let _ = ev;
            Err(CameraError::ControlUnsupported(
                "exposure_compensation".into(),
            ))
        }
    }

    fn set_focus_mode(&self, mode: FocusMode) -> Result<(), CameraError> {
        let av_mode = match mode {
            FocusMode::ContinuousAuto => AVCaptureFocusMode::ContinuousAutoFocus,
            FocusMode::Auto => AVCaptureFocusMode::AutoFocus,
            FocusMode::Manual | FocusMode::Locked => AVCaptureFocusMode::Locked,
        };
        if !unsafe { self.device()?.isFocusModeSupported(av_mode) } {
            return Err(CameraError::ControlUnsupported("focus_mode".into()));
        }
        self.configure_device(|device| {
            unsafe { device.setFocusMode(av_mode) };
            Ok(())
        })
    }

    #[cfg_attr(
        target_os = "macos",
        expect(clippy::unused_self, reason = "the control exists only on iOS")
    )]
    fn set_focus_distance(&self, distance: f32) -> Result<(), CameraError> {
        #[cfg(target_os = "ios")]
        {
            if !unsafe {
                self.device()?
                    .isLockingFocusWithCustomLensPositionSupported()
            } {
                return Err(CameraError::ControlUnsupported("focus_distance".into()));
            }
            self.configure_device(|device| {
                unsafe {
                    device.setFocusModeLockedWithLensPosition_completionHandler(
                        distance.clamp(0.0, 1.0),
                        None,
                    );
                };
                Ok(())
            })
        }
        #[cfg(not(target_os = "ios"))]
        {
            let _ = distance;
            Err(CameraError::ControlUnsupported("focus_distance".into()))
        }
    }

    fn set_focus_point(&self, x: f32, y: f32) -> Result<(), CameraError> {
        if !unsafe { self.device()?.isFocusPointOfInterestSupported() } {
            return Err(CameraError::ControlUnsupported("focus_point".into()));
        }
        self.configure_device(|device| {
            unsafe {
                device.setFocusPointOfInterest(objc2_core_foundation::CGPoint::new(
                    f64::from(x),
                    f64::from(y),
                ));
                device.setFocusMode(AVCaptureFocusMode::AutoFocus);
            }
            Ok(())
        })
    }

    fn set_white_balance_mode(&self, mode: WhiteBalanceMode) -> Result<(), CameraError> {
        let av_mode = match mode {
            WhiteBalanceMode::Auto => AVCaptureWhiteBalanceMode::ContinuousAutoWhiteBalance,
            _ => AVCaptureWhiteBalanceMode::Locked,
        };
        if !unsafe { self.device()?.isWhiteBalanceModeSupported(av_mode) } {
            return Err(CameraError::ControlUnsupported("white_balance".into()));
        }
        self.configure_device(|device| {
            unsafe { device.setWhiteBalanceMode(av_mode) };
            Ok(())
        })
    }

    #[cfg_attr(
        target_os = "macos",
        expect(clippy::unused_self, reason = "the control exists only on iOS")
    )]
    #[cfg_attr(
        target_os = "ios",
        expect(
            clippy::cast_precision_loss,
            reason = "kelvin values are far below 2^24"
        )
    )]
    fn set_white_balance_temperature(&self, kelvin: u32) -> Result<(), CameraError> {
        #[cfg(target_os = "ios")]
        {
            let device = self.device()?.clone();
            let mut gains = unsafe {
                device.deviceWhiteBalanceGainsForTemperatureAndTintValues(
                    AVCaptureWhiteBalanceTemperatureAndTintValues {
                        temperature: kelvin as f32,
                        tint: 0.0,
                    },
                )
            };
            let max_gain = unsafe { device.maxWhiteBalanceGain() };
            gains.redGain = gains.redGain.clamp(1.0, max_gain);
            gains.greenGain = gains.greenGain.clamp(1.0, max_gain);
            gains.blueGain = gains.blueGain.clamp(1.0, max_gain);
            self.configure_device(|device| {
                unsafe {
                    device.setWhiteBalanceModeLockedWithDeviceWhiteBalanceGains_completionHandler(
                        gains, None,
                    );
                };
                Ok(())
            })
        }
        #[cfg(not(target_os = "ios"))]
        {
            let _ = kelvin;
            Err(CameraError::ControlUnsupported(
                "white_balance_temperature".into(),
            ))
        }
    }

    #[cfg_attr(
        target_os = "macos",
        expect(clippy::unused_self, reason = "the control exists only on iOS")
    )]
    fn set_zoom(&self, factor: f32) -> Result<(), CameraError> {
        #[cfg(target_os = "ios")]
        {
            let device = self.device()?.clone();
            let clamped = unsafe {
                f64::from(factor).clamp(
                    device.minAvailableVideoZoomFactor(),
                    device.maxAvailableVideoZoomFactor(),
                )
            };
            self.configure_device(|device| {
                unsafe { device.setVideoZoomFactor(clamped) };
                Ok(())
            })
        }
        #[cfg(not(target_os = "ios"))]
        {
            // macOS doesn't support videoZoomFactor.
            let _ = factor;
            Err(CameraError::ControlUnsupported("zoom".into()))
        }
    }

    fn set_torch(&self, enabled: bool) -> Result<(), CameraError> {
        if !unsafe { self.device()?.hasTorch() } {
            return Err(CameraError::ControlUnsupported("torch".into()));
        }
        self.configure_device(|device| {
            unsafe {
                device.setTorchMode(if enabled {
                    AVCaptureTorchMode::On
                } else {
                    AVCaptureTorchMode::Off
                });
            };
            Ok(())
        })
    }

    #[cfg_attr(
        target_os = "macos",
        expect(clippy::unused_self, reason = "the control exists only on iOS")
    )]
    fn set_dynamic_range(&self, profile: DynamicRangeProfile) -> Result<(), CameraError> {
        #[cfg(target_os = "ios")]
        {
            let session = self
                .capture
                .as_ref()
                .ok_or_else(|| CameraError::OpenFailed("capture session closed".into()))?;
            let movie = self
                .movie_output
                .as_ref()
                .ok_or_else(|| CameraError::OpenFailed("capture session closed".into()))?;
            if unsafe { movie.isRecording() } {
                return Err(CameraError::AlreadyInUse);
            }
            let (format, color_space, codec) = match profile {
                DynamicRangeProfile::Sdr => {
                    let format = self
                        .sdr_format
                        .as_ref()
                        .ok_or_else(|| CameraError::ControlUnsupported("dynamic_range".into()))?;
                    // SAFETY: the color space and codec constants are exported.
                    (
                        format.clone(),
                        AVCaptureColorSpace::sRGB,
                        unsafe { AVVideoCodecTypeH264 }.expect("AVVideoCodecTypeH264 is exported"),
                    )
                }
                DynamicRangeProfile::DolbyVision => {
                    if !objc2::available!(ios = 14.1, ..) {
                        return Err(CameraError::ControlUnsupported("dynamic_range".into()));
                    }
                    let format = self
                        .dolby_vision_format
                        .as_ref()
                        .ok_or_else(|| CameraError::ControlUnsupported("dynamic_range".into()))?;
                    (
                        format.clone(),
                        AVCaptureColorSpace::HLG_BT2020,
                        unsafe { AVVideoCodecTypeHEVC }.expect("AVVideoCodecTypeHEVC is exported"),
                    )
                }
                _ => return Err(CameraError::ControlUnsupported("dynamic_range".into())),
            };
            unsafe { session.beginConfiguration() };
            unsafe { session.setSessionPreset(AVCaptureSessionPresetInputPriority) };
            let result = self.configure_device(|device| {
                unsafe {
                    device.setActiveFormat(&format);
                    device.setActiveColorSpace(color_space);
                    device.setAutomaticallyAdjustsVideoHDREnabled(false);
                    if format.isVideoHDRSupported() {
                        device.setVideoHDREnabled(profile != DynamicRangeProfile::Sdr);
                    }
                }
                Ok(())
            });
            unsafe { session.commitConfiguration() };
            result?;

            // SAFETY: the video connection is the one the movie output uses.
            let video = unsafe { AVMediaTypeVideo }.expect("AVMediaTypeVideo is exported");
            let connection = unsafe { movie.connectionWithMediaType(video) }.ok_or_else(|| {
                CameraError::OpenFailed("movie output has no video connection".into())
            })?;
            let codecs = unsafe { movie.availableVideoCodecTypes() };
            // SAFETY: codec entries are NSStrings (AVVideoCodecType).
            let supported = codecs
                .iter()
                .any(|item| unsafe { msg_send![&**item, isEqualToString: &**codec] });
            if !supported {
                return Err(CameraError::ControlUnsupported("dynamic_range".into()));
            }
            let settings = NSDictionary::<NSString, AnyObject>::from_slices(
                &[unsafe { AVVideoCodecKey }.expect("AVVideoCodecKey is exported")],
                &[codec],
            );
            unsafe { movie.setOutputSettings_forConnection(Some(&settings), &connection) };
            Ok(())
        }
        #[cfg(not(target_os = "ios"))]
        {
            match profile {
                DynamicRangeProfile::Sdr => Ok(()),
                _ => Err(CameraError::ControlUnsupported("dynamic_range".into())),
            }
        }
    }

    #[cfg_attr(
        target_os = "macos",
        expect(clippy::unused_self, reason = "the control exists only on iOS")
    )]
    fn set_stabilization(&self, mode: StabilizationMode) -> Result<(), CameraError> {
        #[cfg(target_os = "ios")]
        {
            let output = self
                .video_output
                .as_ref()
                .ok_or_else(|| CameraError::OpenFailed("capture session closed".into()))?;
            let video = unsafe { AVMediaTypeVideo }.expect("AVMediaTypeVideo is exported");
            let connection = unsafe { output.connectionWithMediaType(video) }
                .ok_or_else(|| CameraError::OpenFailed("video output has no connection".into()))?;
            let av_mode = match mode {
                StabilizationMode::Off => AVCaptureVideoStabilizationMode::Off,
                StabilizationMode::Standard => AVCaptureVideoStabilizationMode::Standard,
                StabilizationMode::Cinematic => AVCaptureVideoStabilizationMode::Cinematic,
            };
            if !unsafe { connection.isVideoStabilizationSupported() } {
                return Err(CameraError::ControlUnsupported("stabilization".into()));
            }
            unsafe { connection.setPreferredVideoStabilizationMode(av_mode) };
            Ok(())
        }
        #[cfg(not(target_os = "ios"))]
        {
            let _ = mode;
            Err(CameraError::ControlUnsupported("stabilization".into()))
        }
    }

    /// Issues one photo capture; the returned receiver carries the encoded
    /// bytes when the framework finishes processing.
    fn take_photo(
        &self,
        raw: bool,
    ) -> Result<futures::channel::oneshot::Receiver<Result<Vec<u8>, CameraError>>, CameraError>
    {
        let output = self
            .photo_output
            .as_ref()
            .ok_or_else(|| CameraError::ControlUnsupported("photo".into()))?;
        let settings = if raw {
            let format = unsafe { output.availableRawPhotoPixelFormatTypes() }
                .firstObject()
                .ok_or_else(|| CameraError::ControlUnsupported("raw_photo".into()))?;
            let pixel_format: u32 = unsafe { msg_send![&*format, unsignedIntValue] };
            unsafe { AVCapturePhotoSettings::photoSettingsWithRawPixelFormatType(pixel_format) }
        } else {
            let codecs = unsafe { output.availablePhotoCodecTypes() };
            let jpeg = unsafe { AVVideoCodecTypeJPEG }.expect("AVVideoCodecTypeJPEG is exported");
            if codecs.iter().any(|codec| *codec == *jpeg) {
                let format = NSDictionary::<NSString, AnyObject>::from_slices(
                    &[unsafe { AVVideoCodecKey }.expect("AVVideoCodecKey is exported")],
                    &[jpeg],
                );
                unsafe { AVCapturePhotoSettings::photoSettingsWithFormat(Some(&format)) }
            } else {
                unsafe { AVCapturePhotoSettings::new() }
            }
        };
        #[cfg(target_os = "ios")]
        #[expect(
            deprecated,
            reason = "kept for parity with the previous implementation"
        )]
        unsafe {
            settings.setHighResolutionPhotoEnabled(true);
        }
        let (delegate, receiver) = PhotoDelegate::new(output);
        unsafe {
            output
                .capturePhotoWithSettings_delegate(&settings, ProtocolObject::from_ref(&*delegate));
        };
        Ok(receiver)
    }

    fn start_recording(&self, path: &Path) -> Result<(), CameraError> {
        let output = self
            .movie_output
            .as_ref()
            .ok_or_else(|| CameraError::ControlUnsupported("recording".into()))?;
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        // Remove an existing file if any.
        let _ = std::fs::remove_file(path);
        *self.recording_start.lock().expect("recording start lock") = Some(Instant::now());
        let delegate = self
            .recording_delegate
            .as_ref()
            .ok_or_else(|| CameraError::ControlUnsupported("recording".into()))?;
        unsafe {
            output.startRecordingToOutputFileURL_recordingDelegate(
                &url,
                ProtocolObject::from_ref(&**delegate),
            );
        };
        Ok(())
    }

    fn stop_recording(&self) -> Result<(), CameraError> {
        let output = self
            .movie_output
            .as_ref()
            .ok_or_else(|| CameraError::ControlUnsupported("recording".into()))?;
        if unsafe { output.isRecording() } {
            unsafe { output.stopRecording() };
        }
        *self.recording_start.lock().expect("recording start lock") = None;
        Ok(())
    }

    fn start_raw_recording(&self, path: &Path) -> Result<(), CameraError> {
        if path.as_os_str().is_empty() {
            return Err(CameraError::OpenFailed("empty raw recording path".into()));
        }
        let mut lock = self.raw_video.lock().expect("raw video lock");
        if lock.file.is_some() {
            return Err(CameraError::AlreadyInUse);
        }
        let _ = std::fs::remove_file(path);
        let file = std::fs::File::create(path)
            .map_err(|error| CameraError::OpenFailed(format!("raw recording file: {error}")))?;
        lock.file = Some(file);
        lock.initial_matrix = None;
        lock.start = Some(Instant::now());
        drop(lock);
        Ok(())
    }

    fn stop_raw_recording(&mut self) {
        let mut lock = self.raw_video.lock().expect("raw video lock");
        drop(lock.file.take());
        lock.initial_matrix = None;
        lock.start = None;
        drop(lock);
    }
}

/// The capture session's resolution preset read back as the applied
/// resolution.
fn preset_resolution(preset: &AVCaptureSessionPreset) -> Resolution {
    let (width, height) = match preset.to_string().as_str() {
        "AVCaptureSessionPreset3840x2160" => (3840, 2160),
        "AVCaptureSessionPreset1920x1080" => (1920, 1080),
        "AVCaptureSessionPreset640x480" => (640, 480),
        "AVCaptureSessionPreset352x288" => (352, 288),
        _ => (1280, 720),
    };
    Resolution { width, height }
}

/// The dimensions of a capture format, for the closest-color-space lookup.
#[cfg(target_os = "ios")]
fn format_dimensions(format: &AVCaptureDeviceFormat) -> (i64, i64) {
    use objc2_core_media::{CMVideoFormatDescription, CMVideoFormatDescriptionGetDimensions};
    let description = unsafe { format.formatDescription() };
    // SAFETY: a capture device's format description is a video format
    // description.
    let description = unsafe { &*(&raw const *description).cast::<CMVideoFormatDescription>() };
    let dimensions = unsafe { CMVideoFormatDescriptionGetDimensions(description) };
    (i64::from(dimensions.width), i64::from(dimensions.height))
}

/// iOS: the device's format closest to the active one's dimensions that
/// supports `color_space`.
#[cfg(target_os = "ios")]
fn closest_format(
    device: &AVCaptureDevice,
    color_space: AVCaptureColorSpace,
) -> Option<Retained<AVCaptureDeviceFormat>> {
    let active = unsafe { device.activeFormat() };
    let (aw, ah) = format_dimensions(&active);
    let formats = unsafe { device.formats() }.to_vec();
    let mut best: Option<Retained<AVCaptureDeviceFormat>> = None;
    let mut best_distance = i64::MAX;
    for format in &formats {
        let spaces = unsafe { format.supportedColorSpaces() };
        let has_space = spaces.iter().any(|space| {
            // SAFETY: color space entries are NSNumbers.
            let value: isize = unsafe { msg_send![&**space, longLongValue] };
            value == color_space.0
        });
        if !has_space {
            continue;
        }
        let (w, h) = format_dimensions(format);
        let distance = (w - aw).abs() + (h - ah).abs();
        if distance < best_distance {
            best_distance = distance;
            best = Some(format.clone());
        }
    }
    best
}

/// Reads the opened device's capabilities, mirroring the fields the previous implementation cached.
#[expect(
    clippy::too_many_lines,
    reason = "capability probing is one flat table; each group reads a different property set"
)]
#[cfg_attr(
    target_os = "ios",
    expect(
        clippy::cast_possible_truncation,
        reason = "zoom factors are far below 2^24"
    )
)]
fn query_capabilities(
    device: &AVCaptureDevice,
    movie_output: Option<&AVCaptureMovieFileOutput>,
    photo_output: Option<&AVCapturePhotoOutput>,
    session: &mut Session,
) -> CameraCapabilities {
    #[cfg(target_os = "ios")]
    let mut stabilization_modes = vec![StabilizationMode::Off];
    #[cfg(not(target_os = "ios"))]
    let stabilization_modes = vec![StabilizationMode::Off];
    #[cfg(target_os = "ios")]
    let mut iso_range = None;
    #[cfg(target_os = "ios")]
    let mut exposure_duration_range = None;
    #[cfg(target_os = "ios")]
    let mut zoom_range = None;
    #[cfg(target_os = "ios")]
    let supports_exposure_compensation: bool;
    #[cfg(target_os = "ios")]
    let supports_dolby_vision: bool;
    #[cfg(target_os = "ios")]
    let supports_raw_photo: bool;
    #[cfg(target_os = "ios")]
    let supports_concurrent: bool;

    #[cfg(target_os = "ios")]
    {
        let format = unsafe { device.activeFormat() };
        let (min_iso, max_iso) = unsafe { (format.minISO(), format.maxISO()) };
        if max_iso > min_iso {
            iso_range = Some((min_iso, max_iso));
        }
        let (min_d, max_d) = unsafe {
            (
                format.minExposureDuration().seconds(),
                format.maxExposureDuration().seconds(),
            )
        };
        if max_d > min_d {
            exposure_duration_range = Some((
                Duration::from_secs_f64(min_d),
                Duration::from_secs_f64(max_d),
            ));
        }
        session.sdr_format = closest_format(device, AVCaptureColorSpace::sRGB);
        // HLG_BT2020 arrived in iOS 14.1, one point release after this
        // module's deployment target.
        session.dolby_vision_format = if objc2::available!(ios = 14.1, ..) {
            closest_format(device, AVCaptureColorSpace::HLG_BT2020)
        } else {
            None
        };
        supports_dolby_vision = session.dolby_vision_format.is_some()
            && movie_output.is_some_and(|output| {
                let hevc =
                    unsafe { AVVideoCodecTypeHEVC }.expect("AVVideoCodecTypeHEVC is exported");
                // SAFETY: codec entries are NSStrings (AVVideoCodecType).
                unsafe { output.availableVideoCodecTypes() }
                    .iter()
                    .any(|codec| unsafe { msg_send![&**codec, isEqualToString: &*hevc] })
            });
        if unsafe {
            format.isVideoStabilizationModeSupported(AVCaptureVideoStabilizationMode::Standard)
        } {
            stabilization_modes.push(StabilizationMode::Standard);
        }
        if unsafe {
            format.isVideoStabilizationModeSupported(AVCaptureVideoStabilizationMode::Cinematic)
        } {
            stabilization_modes.push(StabilizationMode::Cinematic);
        }
        supports_exposure_compensation = unsafe {
            (device.minExposureTargetBias() - device.maxExposureTargetBias()).abs() > f32::EPSILON
        };
        let (min_zoom, max_zoom) = unsafe {
            (
                device.minAvailableVideoZoomFactor() as f32,
                device.maxAvailableVideoZoomFactor() as f32,
            )
        };
        if max_zoom > min_zoom {
            zoom_range = Some((min_zoom, max_zoom));
        }
        supports_concurrent = unsafe { AVCaptureMultiCamSession::isMultiCamSupported() };
        supports_raw_photo = photo_output.is_some_and(|output| {
            unsafe { output.availableRawPhotoPixelFormatTypes() }
                .firstObject()
                .is_some()
        });
    }

    #[cfg(not(target_os = "ios"))]
    {
        let _ = photo_output;
        let _ = movie_output;
        let _ = &mut *session;
    }

    CameraCapabilities {
        resolutions: vec![
            Resolution::UHD,
            Resolution::FULL_HD,
            Resolution::HD,
            Resolution {
                width: 640,
                height: 480,
            },
        ],
        frame_rates: vec![30, 60],
        iso_range: {
            #[cfg(target_os = "ios")]
            {
                iso_range
            }
            #[cfg(not(target_os = "ios"))]
            {
                None
            }
        },
        exposure_duration_range: {
            #[cfg(target_os = "ios")]
            {
                exposure_duration_range
            }
            #[cfg(not(target_os = "ios"))]
            {
                None
            }
        },
        supports_exposure_compensation: {
            #[cfg(target_os = "ios")]
            {
                supports_exposure_compensation
            }
            #[cfg(not(target_os = "ios"))]
            {
                false
            }
        },
        supports_manual_focus: unsafe { device.isFocusModeSupported(AVCaptureFocusMode::Locked) },
        supports_manual_white_balance: unsafe {
            device.isWhiteBalanceModeSupported(AVCaptureWhiteBalanceMode::Locked)
        },
        zoom_range: {
            #[cfg(target_os = "ios")]
            {
                zoom_range
            }
            #[cfg(not(target_os = "ios"))]
            {
                None
            }
        },
        dynamic_ranges: {
            #[cfg(target_os = "ios")]
            let mut ranges = vec![DynamicRangeProfile::Sdr];
            #[cfg(not(target_os = "ios"))]
            let ranges = vec![DynamicRangeProfile::Sdr];
            #[cfg(target_os = "ios")]
            if supports_dolby_vision {
                ranges.push(DynamicRangeProfile::DolbyVision);
            }
            ranges
        },
        supports_dolby_vision: {
            #[cfg(target_os = "ios")]
            {
                supports_dolby_vision
            }
            #[cfg(not(target_os = "ios"))]
            {
                false
            }
        },
        stabilization_modes,
        has_flash: unsafe { device.hasFlash() },
        has_torch: unsafe { device.hasTorch() },
        supports_concurrent_multi_camera: {
            #[cfg(target_os = "ios")]
            {
                supports_concurrent
            }
            #[cfg(not(target_os = "ios"))]
            {
                false
            }
        },
        max_concurrent_cameras: {
            #[cfg(target_os = "ios")]
            if supports_concurrent {
                NonZeroU8::new(2).expect("2 is non-zero")
            } else {
                NonZeroU8::MIN
            }
            #[cfg(not(target_os = "ios"))]
            {
                NonZeroU8::MIN
            }
        },
        uses_system_photo_pipeline: true,
        uses_system_video_pipeline: true,
        supports_raw_photo: {
            #[cfg(target_os = "ios")]
            {
                supports_raw_photo
            }
            #[cfg(not(target_os = "ios"))]
            {
                false
            }
        },
        raw_photo_formats: {
            #[cfg(target_os = "ios")]
            if supports_raw_photo {
                vec![RawPhotoFormat::Dng]
            } else {
                Vec::new()
            }
            #[cfg(not(target_os = "ios"))]
            {
                Vec::new()
            }
        },
        supports_raw_video: true,
        raw_video_formats: vec![RawVideoFormat::Nv12Frames],
    }
}

// ============================================================================
// CameraInner
// ============================================================================

/// Internal camera backend for Apple platforms.
pub struct CameraInner {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    capabilities: CameraCapabilities,
    controls: CameraControls,
    resolution: Resolution,
    frame_receiver: async_channel::Receiver<Result<RawFrame, CameraError>>,
    /// The analysis stream's channel: `Some` only when the camera was opened
    /// with `CameraConfig::analysis`.
    analysis_receiver: Option<async_channel::Receiver<Result<RawFrame, CameraError>>>,
    session: SessionThread,
    recording_mode: Option<RecordingMode>,
    /// Shared with the `Session`; `recording_duration` reads the instant the
    /// session thread wrote instead of asking it.
    recording_start: Arc<Mutex<Option<Instant>>>,
    /// Shared with the `Session` and the frame delegate; `raw_recording_duration`
    /// reads its start instant.
    raw_video: Arc<Mutex<RawVideo>>,
}

impl CameraInner {
    /// List available camera devices.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "device discovery shares the other platforms' Result-returning signature"
    )]
    pub fn list() -> Result<Vec<CameraInfo>, CameraError> {
        let devices = discover_devices();
        let mut infos = Vec::with_capacity(devices.count());
        for index in 0..devices.count() {
            let device = devices.objectAtIndex(index);
            let description = unsafe { device.modelID() }.to_string();
            infos.push(CameraInfo {
                id: unsafe { device.uniqueID() }.to_string(),
                name: unsafe { device.localizedName() }.to_string(),
                description: if description.is_empty() {
                    None
                } else {
                    Some(description)
                },
                is_front_facing: unsafe { device.position() } == AVCaptureDevicePosition::Front,
            });
        }
        Ok(infos)
    }

    /// Open a camera by its ID.
    pub async fn open(
        camera_id: &str,
        config: CameraConfig,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
    ) -> Result<Self, CameraError> {
        let (sender, receiver) = async_channel::bounded(1);
        // The analysis channel exists only when analysis was configured; the
        // callback then also hands each buffer to it, newest-wins.
        let (analysis_sender, analysis_receiver) = if config.analysis.is_some() {
            let (sender, receiver) = async_channel::bounded(1);
            (Some(sender), Some(receiver))
        } else {
            (None, None)
        };

        let camera_id = camera_id.to_string();
        let (session_thread, answer) = {
            let device_for_thread = Arc::clone(&device);
            SessionThread::spawn(move |session| {
                let opened = session.open(
                    &camera_id,
                    sender,
                    &device_for_thread,
                    analysis_sender.map(|sender| AnalysisOutput {
                        sender,
                        clock: crate::clock::StreamClock::new(),
                    }),
                )?;
                let resolution =
                    session.start(config.resolution.width, config.resolution.height)?;
                Ok(OpenedSession {
                    resolution,
                    ..opened
                })
            })?
        };
        let opened = answer
            .await
            .map_err(|_| CameraError::OpenFailed("session thread died".into()))??;
        opened.capabilities.validate()?;

        Ok(Self {
            device,
            queue,
            capabilities: opened.capabilities,
            controls: CameraControls::default(),
            resolution: opened.resolution,
            frame_receiver: receiver,
            analysis_receiver,
            session: session_thread,
            recording_mode: None,
            recording_start: opened.recording_start,
            raw_video: opened.raw_video,
        })
    }

    pub const fn capabilities(&self) -> &CameraCapabilities {
        &self.capabilities
    }

    pub async fn apply_controls(&mut self, controls: &CameraControls) -> Result<(), CameraError> {
        // Exposure
        if let Some(ref exposure) = controls.exposure {
            self.apply_exposure(exposure).await?;
        }

        // Focus
        if let Some(ref focus) = controls.focus {
            self.apply_focus(focus).await?;
        }

        // White balance
        if let Some(ref wb) = controls.white_balance {
            self.apply_white_balance(wb).await?;
        }

        // Zoom
        if let Some(zoom) = controls.zoom {
            if self.capabilities.zoom_range.is_none() {
                return Err(CameraError::ControlUnsupported("zoom".into()));
            }
            self.session
                .ask(move |session| session.set_zoom(zoom.get()))
                .await??;
            self.controls.zoom = Some(zoom);
        }

        // Flash
        if let Some(flash) = controls.flash {
            if !self.capabilities.has_flash && !self.capabilities.has_torch {
                return Err(CameraError::ControlUnsupported("flash".into()));
            }
            if flash == FlashMode::Torch {
                if !self.capabilities.has_torch {
                    return Err(CameraError::ControlUnsupported("torch".into()));
                }
                self.session
                    .ask(|session| session.set_torch(true))
                    .await??;
                self.controls.flash = Some(flash);
                return Ok(());
            }
            // Flash mode applies during photo capture, not on the device;
            // switching away from Torch turns the torch off.
            if self.controls.flash == Some(FlashMode::Torch) {
                let _ = self.session.ask(|session| session.set_torch(false)).await;
            }
            self.controls.flash = Some(flash);
        }

        // Dynamic range
        if let Some(profile) = controls.dynamic_range {
            if !self.capabilities.dynamic_ranges.contains(&profile) {
                return Err(CameraError::ControlUnsupported(format!(
                    "dynamic_range.{profile:?}"
                )));
            }
            self.session
                .ask(move |session| session.set_dynamic_range(profile))
                .await??;
            self.controls.dynamic_range = Some(profile);
        }

        // Stabilization
        if let Some(stabilization) = controls.stabilization {
            if !self
                .capabilities
                .stabilization_modes
                .contains(&stabilization)
            {
                return Err(CameraError::ControlUnsupported(format!(
                    "stabilization {stabilization:?}"
                )));
            }
            self.session
                .ask(move |session| session.set_stabilization(stabilization))
                .await??;
            self.controls.stabilization = Some(stabilization);
        }

        Ok(())
    }

    async fn apply_exposure(&mut self, exposure: &ExposureControl) -> Result<(), CameraError> {
        let mode = exposure.mode;
        self.session
            .ask(move |session| session.set_exposure_mode(mode))
            .await??;

        if exposure.mode == ExposureMode::Manual {
            if let Some(iso) = exposure.iso {
                if let Some((min, max)) = self.capabilities.iso_range {
                    if iso < min || iso > max {
                        return Err(CameraError::ValueOutOfRange(format!(
                            "ISO {iso} not in range [{min}, {max}]"
                        )));
                    }
                } else {
                    return Err(CameraError::ControlUnsupported("iso".into()));
                }
                self.session
                    .ask(move |session| session.set_iso(iso))
                    .await??;
            }

            if let Some(duration) = exposure.duration {
                if let Some((min, max)) = self.capabilities.exposure_duration_range {
                    if duration < min || duration > max {
                        return Err(CameraError::ValueOutOfRange(format!(
                            "exposure duration {duration:?} not in range [{min:?}, {max:?}]"
                        )));
                    }
                } else {
                    return Err(CameraError::ControlUnsupported("exposure_duration".into()));
                }
                self.session
                    .ask(move |session| session.set_exposure_duration(duration))
                    .await??;
            }
        }

        if let Some(ev) = exposure.compensation {
            if !self.capabilities.supports_exposure_compensation {
                return Err(CameraError::ControlUnsupported(
                    "exposure_compensation".into(),
                ));
            }
            self.session
                .ask(move |session| session.set_exposure_compensation(ev))
                .await??;
        }

        self.controls.exposure = Some(exposure.clone());
        Ok(())
    }

    async fn apply_focus(&mut self, focus: &FocusControl) -> Result<(), CameraError> {
        let mode = focus.mode;
        self.session
            .ask(move |session| session.set_focus_mode(mode))
            .await??;

        if let Some(distance) = focus.distance.filter(|_| focus.mode == FocusMode::Manual) {
            if !self.capabilities.supports_manual_focus {
                return Err(CameraError::ControlUnsupported("manual_focus".into()));
            }
            if !(0.0..=1.0).contains(&distance) {
                return Err(CameraError::ValueOutOfRange(format!(
                    "focus distance {distance} not in range [0.0, 1.0]"
                )));
            }
            self.session
                .ask(move |session| session.set_focus_distance(distance))
                .await??;
        }

        if let Some((x, y)) = focus.point_of_interest {
            if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
                return Err(CameraError::ValueOutOfRange(
                    "focus point must be in range [0.0, 1.0]".into(),
                ));
            }
            self.session
                .ask(move |session| session.set_focus_point(x, y))
                .await??;
        }

        self.controls.focus = Some(focus.clone());
        Ok(())
    }

    async fn apply_white_balance(&mut self, wb: &WhiteBalanceControl) -> Result<(), CameraError> {
        let mode = wb.mode;
        self.session
            .ask(move |session| session.set_white_balance_mode(mode))
            .await??;

        // Set temperature for presets or manual
        let temperature = match wb.mode {
            WhiteBalanceMode::Auto => None,
            WhiteBalanceMode::Manual => wb.temperature,
            WhiteBalanceMode::Daylight => Some(5600),
            WhiteBalanceMode::Cloudy => Some(6500),
            WhiteBalanceMode::Tungsten => Some(3200),
            WhiteBalanceMode::Fluorescent => Some(4000),
        };

        if let Some(kelvin) = temperature {
            if !self.capabilities.supports_manual_white_balance {
                return Err(CameraError::ControlUnsupported(
                    "manual_white_balance".into(),
                ));
            }
            self.session
                .ask(move |session| session.set_white_balance_temperature(kelvin))
                .await??;
        }

        self.controls.white_balance = Some(wb.clone());
        Ok(())
    }

    pub const fn controls(&self) -> &CameraControls {
        &self.controls
    }

    pub const fn resolution(&self) -> Resolution {
        self.resolution
    }

    pub fn frames(&self) -> impl futures::Stream<Item = Result<Frame, CameraError>> + '_ {
        let device = Arc::clone(&self.device);
        let receiver = self.frame_receiver.clone();

        futures::stream::unfold((device, receiver), |(device, receiver)| async move {
            let raw = receiver.recv().await.ok()?;
            let frame = raw.map(|raw| capture::build_frame(&device, raw));
            Some((frame, (device, receiver)))
        })
    }

    /// The capture output's buffers also carry the analysis stream: each
    /// analysis frame retains the `CVPixelBuffer` a preview frame's planes
    /// alias.
    pub fn analysis_frames(
        &self,
    ) -> impl futures::Stream<Item = Result<AnalysisFrame, CameraError>> + '_ {
        crate::analysis::stream(self.analysis_receiver.clone(), |raw| {
            raw.map(capture::build_analysis_frame)
        })
    }

    pub async fn capture_photo(&self) -> Result<Photo, CameraError> {
        let (tx, rx) = futures::channel::oneshot::channel();
        self.session.send(move |session| {
            let _ = tx.send(session.take_photo(false));
        })?;
        let receiver = rx
            .await
            .map_err(|_| CameraError::CaptureFailed("session thread died".into()))??;
        let encoded = receiver
            .await
            .map_err(|_| CameraError::CaptureFailed("photo delegate was dropped".into()))??;

        let dynamic = image::load_from_memory(&encoded).map_err(|error| {
            CameraError::CaptureFailed(format!("failed to decode captured photo: {error}"))
        })?;
        let rgba = dynamic.to_rgba8();
        let width = rgba.width();
        let height = rgba.height();
        let data = rgba.into_raw();

        // Create GPU texture
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("CameraPhoto"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Upload to GPU
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data.as_slice(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        Ok(Photo {
            texture,
            width,
            height,
        })
    }

    pub async fn capture_raw_photo(&self) -> Result<RawPhoto, CameraError> {
        if !self.capabilities.supports_raw_photo {
            return Err(CameraError::ControlUnsupported("raw_photo".into()));
        }
        let (tx, rx) = futures::channel::oneshot::channel();
        self.session.send(move |session| {
            let _ = tx.send(session.take_photo(true));
        })?;
        let receiver = rx
            .await
            .map_err(|_| CameraError::CaptureFailed("session thread died".into()))??;
        let data = receiver
            .await
            .map_err(|_| CameraError::CaptureFailed("photo delegate was dropped".into()))??;

        Ok(RawPhoto {
            data,
            width: self.resolution.width,
            height: self.resolution.height,
            format: RawPhotoFormat::Dng,
        })
    }

    pub async fn start_recording(&mut self, path: &Path) -> Result<(), CameraError> {
        if self.recording_mode.is_some() {
            return Err(CameraError::AlreadyInUse);
        }
        let path_buf = path.to_path_buf();
        self.session
            .ask(move |session| session.start_recording(&path_buf))
            .await??;
        self.recording_mode = Some(RecordingMode::Standard);
        Ok(())
    }

    pub async fn stop_recording(&mut self) -> Result<(), CameraError> {
        match self.recording_mode {
            Some(RecordingMode::Standard) => {
                self.session
                    .ask(|session| session.stop_recording())
                    .await??;
                self.recording_mode = None;
                Ok(())
            }
            Some(RecordingMode::Raw) => Err(CameraError::RecordingError(
                "raw recording active; call stop_raw_recording".into(),
            )),
            None => Ok(()),
        }
    }

    pub fn recording_duration(&self) -> Duration {
        match self.recording_mode {
            Some(RecordingMode::Standard) => self
                .recording_start
                .lock()
                .expect("recording start lock")
                .map_or(Duration::ZERO, |start| start.elapsed()),
            _ => Duration::ZERO,
        }
    }

    pub async fn start_raw_recording(&mut self, path: &Path) -> Result<(), CameraError> {
        if !self.capabilities.supports_raw_video {
            return Err(CameraError::ControlUnsupported("raw_video".into()));
        }
        if self.recording_mode.is_some() {
            return Err(CameraError::AlreadyInUse);
        }
        let path_buf = path.to_path_buf();
        self.session
            .ask(move |session| session.start_raw_recording(&path_buf))
            .await??;
        self.recording_mode = Some(RecordingMode::Raw);
        Ok(())
    }

    pub async fn stop_raw_recording(&mut self) -> Result<(), CameraError> {
        match self.recording_mode {
            Some(RecordingMode::Raw) => {
                self.session.ask(Session::stop_raw_recording).await?;
                self.recording_mode = None;
                Ok(())
            }
            Some(RecordingMode::Standard) => Err(CameraError::RecordingError(
                "standard recording active; call stop_recording".into(),
            )),
            None => Ok(()),
        }
    }

    pub fn raw_recording_duration(&self) -> Duration {
        match self.recording_mode {
            Some(RecordingMode::Raw) => self
                .raw_video
                .lock()
                .expect("raw video lock")
                .start
                .map_or(Duration::ZERO, |start| start.elapsed()),
            _ => Duration::ZERO,
        }
    }
    /// Ends an active standard recording without waiting for the file
    /// output, for `Drop` paths that cannot await.
    pub fn abandon_recording(&mut self) {
        if matches!(self.recording_mode, Some(RecordingMode::Standard)) {
            let _ = self.session.send(|session| {
                let _ = session.stop_recording();
            });
        }
        self.recording_mode = None;
    }

    /// Ends an active raw recording without waiting, for `Drop` paths that
    /// cannot await.
    pub fn abandon_raw_recording(&mut self) {
        if matches!(self.recording_mode, Some(RecordingMode::Raw)) {
            let _ = self.session.send(Session::stop_raw_recording);
        }
        self.recording_mode = None;
    }
}

impl Drop for CameraInner {
    fn drop(&mut self) {
        match self.recording_mode {
            Some(RecordingMode::Standard) => self.abandon_recording(),
            Some(RecordingMode::Raw) => self.abandon_raw_recording(),
            None => {}
        }
        // Dropping the command channel ends the session thread, which runs
        // `Session::stop` — `stopRunning` and the teardown — on that thread.
        // The JoinHandle detaches.
    }
}
