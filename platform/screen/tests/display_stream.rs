//! Display streams on a virtual display this test creates, so nothing on the
//! machine's own displays is captured. Needs the Screen Recording grant for
//! the process responsible for the test binary.

#![cfg(target_os = "macos")]

use core::num::NonZeroU32;
use core::time::Duration;
use std::ffi::{CStr, c_void};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use futures::future::{self, Either};
use futures::{Stream, StreamExt};
use objc2::msg_send;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2_core_foundation::{CFRunLoop, CGSize, kCFRunLoopDefaultMode};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayChangeSummaryFlags, CGDisplayRegisterReconfigurationCallback,
    CGDisplayRemoveReconfigurationCallback, CGError, CGGetOnlineDisplayList,
};
use objc2_foundation::{NSArray, NSString};
use waterkit_screen::{
    DisplayStream, DisplayStreamConfig, Error, PixelBuffer, PixelFormat, ScreenInfo, ScreenStream,
    StreamConfig, screens,
};

/// How long the capture service gets to deliver a frame.
const FRAME_DEADLINE: Duration = Duration::from_secs(5);

fn class(name: &CStr) -> &'static AnyClass {
    AnyClass::get(name)
        .unwrap_or_else(|| panic!("{} is unavailable on this macOS", name.to_string_lossy()))
}

/// The displays `CoreGraphics` reports online.
fn online_displays() -> Vec<u32> {
    let mut ids = [0_u32; 32];
    let mut count = 0_u32;
    // SAFETY: `ids` holds 32 entries and `count` is a valid out pointer.
    let status = unsafe { CGGetOnlineDisplayList(32, ids.as_mut_ptr(), &raw mut count) };
    assert_eq!(status, CGError::Success, "CGGetOnlineDisplayList failed");
    ids.into_iter().take(count as usize).collect()
}

/// How long a new virtual display gets to go online.
const ONLINE_DEADLINE: Duration = Duration::from_secs(5);

/// The displays `CoreGraphics` reported added since `watch`.
struct DisplayAdditions {
    added: Box<Mutex<Vec<u32>>>,
}

unsafe extern "C-unwind" fn record_addition(
    display: CGDirectDisplayID,
    flags: CGDisplayChangeSummaryFlags,
    added: *mut c_void,
) {
    if flags.contains(CGDisplayChangeSummaryFlags::AddFlag) {
        // SAFETY: `added` is the `Mutex` `DisplayAdditions` registered, which
        // outlives the callback's registration.
        let added = unsafe { &*added.cast::<Mutex<Vec<u32>>>() };
        added
            .lock()
            .expect("the addition list is not poisoned")
            .push(display);
    }
}

impl DisplayAdditions {
    fn watch() -> Self {
        let added = Box::new(Mutex::new(Vec::new()));
        let info = (&raw const *added).cast_mut().cast::<c_void>();
        // SAFETY: the callback matches the registered signature and `info`
        // stays valid until `drop` unregisters it.
        let status =
            unsafe { CGDisplayRegisterReconfigurationCallback(Some(record_addition), info) };
        assert_eq!(
            status,
            CGError::Success,
            "registering the display callback failed"
        );
        Self { added }
    }

    /// Waits for the add event of `id`, or for it to be listed online
    /// already. The reconfiguration callback is delivered through the run
    /// loop, which the test thread runs while it waits.
    fn wait_until_online(&self, id: u32) {
        let deadline = Instant::now() + ONLINE_DEADLINE;
        loop {
            let added = self
                .added
                .lock()
                .expect("the addition list is not poisoned")
                .contains(&id);
            let online = online_displays();
            if added || online.contains(&id) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "CoreGraphics does not list virtual display {id} as online (online: {online:?})"
            );
            CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, 0.05, false);
        }
    }
}

impl Drop for DisplayAdditions {
    fn drop(&mut self) {
        let info = (&raw const *self.added).cast_mut().cast::<c_void>();
        // SAFETY: the registration made in `watch` is removed with the same
        // callback and pointer.
        let status = unsafe { CGDisplayRemoveReconfigurationCallback(Some(record_addition), info) };
        assert!(
            status == CGError::Success || std::thread::panicking(),
            "unregistering the display callback failed"
        );
    }
}

/// A `CGVirtualDisplay` (private `CoreGraphics` API) of `width`x`height`
/// points at 2x backing scale. Dropping it removes the display.
struct VirtualDisplay {
    _display: Retained<AnyObject>,
    id: u32,
}

impl VirtualDisplay {
    fn create(width: u32, height: u32) -> Self {
        // Registered first: the display goes online asynchronously after
        // `applySettings:`.
        let additions = DisplayAdditions::watch();
        // SAFETY: the messages match the private classes' selectors; every
        // object is owned by a `Retained`.
        let (display, id) = unsafe {
            let descriptor: Retained<AnyObject> =
                msg_send![class(c"CGVirtualDisplayDescriptor"), new];
            let name = NSString::from_str("waterkit display stream test");
            let _: () = msg_send![&descriptor, setName: &*name];
            let _: () = msg_send![&descriptor, setMaxPixelsWide: width * 2];
            let _: () = msg_send![&descriptor, setMaxPixelsHigh: height * 2];
            // ~110 dpi, a typical desktop panel.
            let millimetres = CGSize::new(
                f64::from(width) / 110.0 * 25.4,
                f64::from(height) / 110.0 * 25.4,
            );
            let _: () = msg_send![&descriptor, setSizeInMillimeters: millimetres];
            let _: () = msg_send![&descriptor, setVendorID: 0x610_u32];
            let _: () = msg_send![&descriptor, setProductID: 0xa08_u32];
            let serial = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("the clock is past the epoch")
                .subsec_nanos()
                | 1;
            let _: () = msg_send![&descriptor, setSerialNum: serial];

            let allocated: Allocated<AnyObject> = msg_send![class(c"CGVirtualDisplay"), alloc];
            let display: Retained<AnyObject> =
                msg_send![allocated, initWithDescriptor: &*descriptor];

            // A HiDPI mode is sized in points; its pixels are twice that,
            // within the descriptor's maximum.
            let settings: Retained<AnyObject> = msg_send![class(c"CGVirtualDisplaySettings"), new];
            let _: () = msg_send![&settings, setHiDPI: 1_u32];
            let allocated: Allocated<AnyObject> = msg_send![class(c"CGVirtualDisplayMode"), alloc];
            let mode: Retained<AnyObject> = msg_send![
                allocated,
                initWithWidth: width,
                height: height,
                refreshRate: 60.0_f64
            ];
            let modes = NSArray::from_retained_slice(&[mode]);
            let _: () = msg_send![&settings, setModes: &*modes];
            let applied: bool = msg_send![&display, applySettings: &*settings];
            assert!(applied, "CoreGraphics refused the virtual display settings");
            let id: u32 = msg_send![&display, displayID];
            (display, id)
        };
        assert_ne!(id, 0, "CoreGraphics assigned the virtual display no id");
        additions.wait_until_online(id);
        Self {
            _display: display,
            id,
        }
    }
}

const fn size(pixels: u32) -> NonZeroU32 {
    NonZeroU32::new(pixels).expect("a test size is non-zero")
}

/// The stream's next item, or `None` when it yields nothing in time.
async fn within_deadline<S: Stream + Unpin>(items: &mut S) -> Option<Option<S::Item>> {
    match future::select(items.next(), futures_timer::Delay::new(FRAME_DEADLINE)).await {
        Either::Left((item, _)) => Some(item),
        Either::Right(((), _)) => None,
    }
}

/// The next frame, failing the test when none arrives in time.
async fn next_frame(frames: &mut (impl Stream<Item = PixelBuffer> + Unpin)) -> PixelBuffer {
    within_deadline(frames)
        .await
        .unwrap_or_else(|| panic!("no frame arrived within {FRAME_DEADLINE:?}"))
        .expect("the frame stream ended")
}

fn assert_frame(frame: &PixelBuffer, width: u32, height: u32, format: PixelFormat) {
    assert_eq!((frame.width(), frame.height()), (width, height));
    assert_eq!(frame.pixel_format(), format);
    let surface = frame.io_surface();
    assert_eq!(
        (surface.width(), surface.height()),
        (width as usize, height as usize),
        "the pixel buffer's IOSurface holds the frame"
    );
}

const fn assert_send<T: Send>(value: T) -> T {
    value
}

async fn display_stream_delivers_requested_frames(display_id: u32) {
    let config = DisplayStreamConfig::new(size(1280), size(800))
        .frame_rate(size(30))
        .pixel_format(PixelFormat::Bgra8);
    let mut stream = assert_send(DisplayStream::new(display_id, config))
        .await
        .expect("a display stream opens on the virtual display");
    let mut frames = Box::pin(stream.frames());

    assert_send(stream.resume()).await.expect("capture starts");
    assert_frame(
        &next_frame(&mut frames).await,
        1280,
        800,
        PixelFormat::Bgra8,
    );

    assert_send(stream.pause()).await.expect("capture pauses");
    stream.resume().await.expect("capture resumes");
    assert_frame(
        &next_frame(&mut frames).await,
        1280,
        800,
        PixelFormat::Bgra8,
    );

    drop(stream);
    // Frames captured before the drop may still be queued; then the frames
    // end.
    loop {
        match within_deadline(&mut frames).await {
            Some(Some(_queued)) => {}
            Some(None) => break,
            None => panic!("the frames did not end after their DisplayStream dropped"),
        }
    }

    let config =
        DisplayStreamConfig::new(size(640), size(400)).pixel_format(PixelFormat::Ycbcr420Video);
    let mut stream = DisplayStream::new(display_id, config)
        .await
        .expect("a second display stream opens");
    let mut frames = Box::pin(stream.frames());
    stream.resume().await.expect("capture starts");
    assert_frame(
        &next_frame(&mut frames).await,
        640,
        400,
        PixelFormat::Ycbcr420Video,
    );
}

/// The existing wgpu stream on the same display. Its `try_next_frame` does
/// not wait, so the frame is polled for.
#[expect(
    clippy::future_not_send,
    reason = "the wgpu ScreenStream's capturer is not Send on macOS"
)]
async fn wgpu_stream_delivers_frames(info: &ScreenInfo) {
    let adapter = wgpu::Instance::default()
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .expect("a GPU adapter");
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .expect("a GPU device");
    let stream = ScreenStream::start(
        info,
        Arc::new(device),
        Arc::new(queue),
        &StreamConfig::default(),
    )
    .await
    .expect("the wgpu stream starts on the virtual display");
    let deadline = std::time::Instant::now() + FRAME_DEADLINE;
    let frame = loop {
        if let Some(frame) = stream.try_next_frame() {
            break frame;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the wgpu stream delivered no frame within {FRAME_DEADLINE:?}"
        );
        futures_timer::Delay::new(Duration::from_millis(20)).await;
    };
    assert!(frame.width() > 0 && frame.height() > 0);
    assert_eq!(
        (frame.texture().width(), frame.texture().height()),
        (frame.width(), frame.height())
    );
}

/// One virtual display serves every capture check: creating several at once
/// makes `CoreGraphics` refuse some of them.
#[test]
fn a_virtual_display_streams_frames() {
    let display = VirtualDisplay::create(640, 400);
    let info = screens()
        .expect("displays are listed")
        .into_iter()
        .find(|screen| screen.id() == display.id)
        .expect("the virtual display is listed");
    pollster::block_on(async {
        display_stream_delivers_requested_frames(display.id).await;
        wgpu_stream_delivers_frames(&info).await;
    });
}

#[test]
fn an_unknown_display_is_not_found() {
    let result = pollster::block_on(DisplayStream::new(
        u32::MAX,
        DisplayStreamConfig::new(size(64), size(64)),
    ));
    assert!(
        matches!(result, Err(Error::MonitorNotFound)),
        "got {result:?}"
    );
}
