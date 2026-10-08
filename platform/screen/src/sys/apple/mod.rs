//! Apple platform implementation (macOS/iOS).
//!
//! Screenshots render through `ScreenCaptureKit` on macOS and `UIKit`'s
//! image renderer on iOS; brightness goes through `IOKit`'s
//! `IODisplayConnect` service on macOS and `UIScreen` on iOS. Screen
//! streaming is macOS-only and owns its `SCStream` through the returned
//! capturer object — no global state.

use crate::frame::ScreenFrame;
use crate::screenshot::ImageFormat;
use crate::stream::StreamConfig;
use crate::{Error, ScreenInfo, Screenshot};
use std::sync::Arc;
use wgpu::{Device, Queue};
#[cfg(target_os = "macos")]
use wgpu::{Extent3d, TextureDimension, TextureFormat, TextureUsages};

use objc2_core_foundation::{CFMutableData, CFString};
use objc2_core_graphics::CGImage;
use objc2_image_io::CGImageDestination;

/// Encodes a `CGImage` as `format` (0 = PNG, 1 = AVIF, 2 = HEIF).
fn encode_image(image: &CGImage, format: u8) -> Option<Vec<u8>> {
    let uti = match format {
        1 => "public.avif",
        2 => "public.heic",
        _ => "public.png",
    };
    let data = CFMutableData::new(None, 0)?;
    // SAFETY: `data` is a valid CFMutableData and `uti` a known UTI string.
    let destination =
        unsafe { CGImageDestination::with_data(&data, &CFString::from_str(uti), 1, None) }?;
    // SAFETY: `destination` and `image` are valid objects.
    unsafe { destination.add_image(image, None) };
    // SAFETY: `destination` is valid.
    if unsafe { destination.finalize() } {
        Some(data.to_vec())
    } else {
        None
    }
}

// ============================================================================
// iOS
// ============================================================================

#[cfg(target_os = "ios")]
mod ios {
    use core::ptr::NonNull;
    use std::sync::mpsc;

    use block2::RcBlock;
    use dispatch2::DispatchQueue;
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_ui_kit::{
        UIApplication, UIGraphicsImageRenderer, UIGraphicsImageRendererContext, UIScreen,
    };

    /// Runs `work` on the main thread, dispatching through the main queue
    /// when the caller is on another thread.
    fn on_main_thread<R: Send>(work: impl FnOnce(MainThreadMarker) -> R + Send) -> R {
        if let Some(mtm) = MainThreadMarker::new() {
            return work(mtm);
        }
        let mut result = None;
        DispatchQueue::main().exec_sync(|| {
            let mtm = MainThreadMarker::new().expect("the main queue only runs on the main thread");
            result = Some(work(mtm));
        });
        result.expect("a synchronous block on the main queue ran to completion")
    }

    /// Encodes a rendered `UIImage` per `format` (0 = PNG, 1 = AVIF, 2 = HEIF).
    fn encode_ui_image(image: &objc2_ui_kit::UIImage, format: u8) -> Option<Vec<u8>> {
        match format {
            1 | 2 => {
                // SAFETY: `image` is a valid UIImage; `CGImage` borrows its
                // backing store.
                let cg_image = unsafe { image.CGImage() }?;
                super::encode_image(&cg_image, format)
            }
            _ => image.png_representation().map(|data| data.to_vec()),
        }
    }

    /// Capture the key window and return encoded image data.
    pub fn capture_screenshot(format: u8) -> Vec<u8> {
        let (sender, receiver) = mpsc::channel();
        DispatchQueue::main().exec_sync(move || {
            let mtm =
                MainThreadMarker::new().expect("the main queue only runs on the main thread");
            let application = UIApplication::sharedApplication(mtm);
            #[expect(
                deprecated,
                reason = "matches the previous implementation, which captured the deprecated key window list"
            )]
            let window = application
                .windows()
                .iter()
                .find(|window| window.isKeyWindow());
            let Some(window) = window else {
                return;
            };
            let bounds = window.bounds();
            let renderer = UIGraphicsImageRenderer::initWithBounds(
                UIGraphicsImageRenderer::alloc(),
                bounds,
            );
            let block = RcBlock::new(
                move |_context: NonNull<UIGraphicsImageRendererContext>| {
                    let _ = window.drawViewHierarchyInRect_afterScreenUpdates(bounds, true);
                },
            );
            // SAFETY: `block` is a valid drawing-actions block; the call is
            // synchronous on the main thread.
            let image = unsafe { renderer.imageWithActions(RcBlock::as_ptr(&block)) };
            if let Some(data) = encode_ui_image(&image, format) {
                let _ = sender.send(data);
            }
        });
        receiver.recv().unwrap_or_default()
    }

    pub fn get_screen_brightness() -> f32 {
        #[expect(
            deprecated,
            reason = "UIScreen.mainScreen is the API the previous implementation used for brightness"
        )]
        #[expect(
            clippy::cast_possible_truncation,
            reason = "UIScreen.brightness is a 0-1 CGFloat; a c_float copy loses no meaning"
        )]
        on_main_thread(|mtm| UIScreen::mainScreen(mtm).brightness() as f32)
    }

    pub fn set_screen_brightness(value: f32) -> bool {
        #[expect(
            deprecated,
            reason = "UIScreen.mainScreen is the API the previous implementation used for brightness"
        )]
        on_main_thread(|mtm| {
            UIScreen::mainScreen(mtm).setBrightness(f64::from(value));
        });
        true
    }
}

// ============================================================================
// macOS
// ============================================================================

#[cfg(target_os = "macos")]
mod macos {
    use core::ptr::NonNull;
    use std::sync::mpsc::{self, Sender};
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::Duration;

    use core::ffi::CStr;

    use block2::RcBlock;
    use dispatch2::{DispatchQoS, DispatchQueue, GlobalQueueIdentifier};
    use objc2::rc::{Retained, Weak};
    use objc2::runtime::{NSObject, ProtocolObject};
    use objc2::{AnyThread, DefinedClass, available, define_class, msg_send};
    use objc2_core_foundation::{
        CFDictionary, CFRetained, CFString, CFType, CGPoint, CGRect, CGSize,
    };
    use objc2_core_graphics::CGDirectDisplayID;
    use objc2_core_image::{CIContext, CIImage};
    use objc2_core_media::{CMSampleBuffer, CMTime};
    use objc2_core_video::{
        CVPixelBufferGetHeight, CVPixelBufferGetIOSurface, CVPixelBufferGetWidth,
        kCVPixelFormatType_32BGRA,
    };
    use objc2_foundation::{NSArray, NSError, NSObjectProtocol};
    #[expect(
        deprecated,
        reason = "kIOMasterPortDefault is the documented entry point on macOS < 12"
    )]
    use objc2_io_kit::kIOMasterPortDefault;
    use objc2_io_kit::{
        IODisplayCreateInfoDictionary, IODisplayGetFloatParameter, IODisplaySetFloatParameter,
        IOIteratorNext, IOObjectRelease, IOServiceGetMatchingServices, IOServiceMatching,
        io_iterator_t, io_object_t, kDisplayProductID, kDisplaySerialNumber, kDisplayVendorID,
        kIODisplayBrightnessKey, kIODisplayOnlyPreferredName, kIOMainPortDefault,
    };
    use objc2_io_surface::IOSurfaceRef;
    use objc2_screen_capture_kit::{
        SCContentFilter, SCScreenshotManager, SCShareableContent, SCStream, SCStreamConfiguration,
        SCStreamDelegate, SCStreamOutput, SCStreamOutputType,
    };

    // Not bound by `objc2-core-graphics` 0.3.x: the `CGDirectDisplay.h` vendor /
    // model / serial queries used to match the main display's IODisplay service.
    unsafe extern "C" {
        fn CGDisplayVendorNumber(display: CGDirectDisplayID) -> u32;
        fn CGDisplayModelNumber(display: CGDirectDisplayID) -> u32;
        fn CGDisplaySerialNumber(display: CGDirectDisplayID) -> u32;
    }

    // ------------------------------------------------------------------
    // Screenshot
    // ------------------------------------------------------------------

    /// Waits on `sender`, used like the dispatch semaphores in the previous
    /// implementation: returns `None` when the producer never answers.
    fn wait<T>(receiver: &mpsc::Receiver<T>, timeout: Duration) -> Option<T> {
        receiver.recv_timeout(timeout).ok()
    }

    /// Encodes the first `.screen` frame an `SCStream` produces, then resolves
    /// its sender; also resolves on `didStopWithError`. Used for the pre-14.0
    /// screenshot fallback.
    #[derive(Debug)]
    pub struct SingleFrameIvars {
        format: u8,
        sender: Mutex<Option<Sender<Option<Vec<u8>>>>>,
    }

    define_class!(
        // SAFETY:
        // - The superclass NSObject does not have any subclassing requirements.
        // - `SingleFrameHandler` does not implement `Drop`.
        #[unsafe(super(NSObject))]
        #[thread_kind = AnyThread]
        #[name = "WaterkitSingleFrameHandler"]
        #[ivars = SingleFrameIvars]
        #[derive(Debug)]
        pub struct SingleFrameHandler;

        unsafe impl NSObjectProtocol for SingleFrameHandler {}

        unsafe impl SCStreamOutput for SingleFrameHandler {
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
                // SAFETY: `sample_buffer` is a live frame delivered by
                // ScreenCaptureKit on the sample handler queue.
                let Some(image_buffer) = (unsafe { sample_buffer.image_buffer() }) else {
                    return;
                };
                let result = (|| {
                    let (width, height) = (
                        CVPixelBufferGetWidth(&image_buffer),
                        CVPixelBufferGetHeight(&image_buffer),
                    );
                    // SAFETY: `image_buffer` is a valid CVImageBuffer.
                    let ci_image = unsafe { CIImage::imageWithCVImageBuffer(&image_buffer) };
                    // SAFETY: plain `[[CIContext alloc] init]`.
                    let context = unsafe { CIContext::new() };
                    // SAFETY: `ci_image` is a valid CIImage; the rect is in
                    // pixels like the previous implementation.
                    #[expect(
                        clippy::cast_precision_loss,
                        reason = "pixel dimensions are far below f64's exact integer range"
                    )]
                    let cg_image = unsafe {
                        context.createCGImage_fromRect(
                            &ci_image,
                            CGRect::new(
                                CGPoint::new(0.0, 0.0),
                                CGSize::new(width as f64, height as f64),
                            ),
                        )
                    }?;
                    super::encode_image(&cg_image, self.ivars().format)
                })();
                let sender = self.ivars().sender.lock().unwrap().take();
                if let Some(sender) = sender {
                    let _ = sender.send(result);
                }
            }
        }

        unsafe impl SCStreamDelegate for SingleFrameHandler {
            #[unsafe(method(stream:didStopWithError:))]
            fn did_stop_with_error(&self, _stream: &SCStream, _error: &NSError) {
                let sender = self.ivars().sender.lock().unwrap().take();
                if let Some(sender) = sender {
                    let _ = sender.send(None);
                }
            }
        }
    );

    impl SingleFrameHandler {
        fn alloc_with(format: u8, sender: Sender<Option<Vec<u8>>>) -> Retained<Self> {
            let this = Self::alloc().set_ivars(SingleFrameIvars {
                format,
                sender: Mutex::new(Some(sender)),
            });
            // SAFETY: `this` is a freshly allocated handler and `NSObject`'s
            // `init` has no additional requirements.
            unsafe { msg_send![super(this), init] }
        }
    }

    /// Single-frame capture through an `SCStream` (the pre-macOS 14 path).
    fn capture_with_stream(
        filter: &SCContentFilter,
        config: &SCStreamConfiguration,
        format: u8,
    ) -> Option<Vec<u8>> {
        let (sender, receiver) = mpsc::channel();
        let handler = SingleFrameHandler::alloc_with(format, sender);
        // SAFETY: `filter`/`config`/`handler` are valid objects; the stream
        // owns its configuration copy.
        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                filter,
                config,
                Some(ProtocolObject::from_ref(&*handler)),
            )
        };
        // SAFETY: `handler` conforms to SCStreamOutput; the main queue is a
        // valid sample handler queue, exactly as before.
        unsafe {
            stream.addStreamOutput_type_sampleHandlerQueue_error(
                ProtocolObject::from_ref(&*handler),
                SCStreamOutputType::Screen,
                Some(DispatchQueue::main()),
            )
        }
        .ok()?;
        // SAFETY: the completion block ignores its error argument.
        let empty = RcBlock::new(|_error: *mut NSError| {});
        unsafe { stream.startCaptureWithCompletionHandler(Some(&empty)) };
        let data = wait(&receiver, Duration::from_secs(2)).flatten();
        // SAFETY: stopping an idle or running stream with no completion is
        // always valid.
        unsafe { stream.stopCaptureWithCompletionHandler(None) };
        data
    }

    /// Capture the primary screen and return encoded image data.
    pub fn capture_screenshot(format: u8) -> Vec<u8> {
        if !available!(macos = 12.3, ..) {
            // ScreenCaptureKit is required; CGWindowListCreateImage was
            // obsoleted in macOS 15 and no longer compiles.
            return Vec::new();
        }
        let (sender, receiver) = mpsc::channel::<Vec<u8>>();
        let block = RcBlock::new(
            move |content: *mut SCShareableContent, _error: *mut NSError| {
                // Dropping `sender` without sending resolves the wait as an
                // empty result, like the semaphore's nil-result path.
                // SAFETY: `content` is an autoreleased object valid for the
                // duration of the callback.
                let Some(content) = (unsafe { content.as_ref() }) else {
                    return;
                };
                // SAFETY: `content` is a live SCShareableContent.
                let displays = unsafe { content.displays() };
                let Some(display) = displays.iter().next() else {
                    return;
                };
                let empty: Retained<NSArray<objc2_screen_capture_kit::SCWindow>> = NSArray::new();
                // SAFETY: `display` and the (empty) exclusion list are valid.
                let filter = unsafe {
                    SCContentFilter::initWithDisplay_excludingWindows(
                        SCContentFilter::alloc(),
                        &display,
                        &empty,
                    )
                };
                // SAFETY: `SCStreamConfiguration` is a plain data object.
                let config = unsafe { SCStreamConfiguration::new() };
                // SAFETY: setters on a live configuration object.
                unsafe {
                    config.setWidth(usize::try_from(display.width()).expect("width >= 0"));
                    config.setHeight(usize::try_from(display.height()).expect("height >= 0"));
                    config.setPixelFormat(kCVPixelFormatType_32BGRA);
                    config.setShowsCursor(true);
                }
                if available!(macos = 14.0, ..) {
                    let captured = {
                        let sender = sender.clone();
                        RcBlock::new(
                            move |image: *mut objc2_core_graphics::CGImage,
                                  _error: *mut NSError| {
                                // SAFETY: `image` is valid for the duration of
                                // the callback.
                                if let Some(data) = unsafe { image.as_ref() }
                                    .and_then(|image| super::encode_image(image, format))
                                {
                                    let _ = sender.send(data);
                                }
                            },
                        )
                    };
                    // SAFETY: `filter`/`config` are valid; `captured` is a
                    // well-typed completion block.
                    unsafe {
                        SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
                            &filter,
                            &config,
                            Some(&captured),
                        );
                    };
                } else if let Some(data) = capture_with_stream(&filter, &config, format) {
                    let _ = sender.send(data);
                }
            },
        );
        // SAFETY: `block` is a well-typed completion block.
        unsafe {
            SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
                false,
                true,
                &block,
            );
        };
        wait(&receiver, Duration::from_secs(5)).unwrap_or_default()
    }

    // ------------------------------------------------------------------
    // Brightness (IODisplayConnect float parameters)
    // ------------------------------------------------------------------

    /// Releases an `IOKit` object handle on drop.
    struct IoObject(io_object_t);

    impl Drop for IoObject {
        fn drop(&mut self) {
            IOObjectRelease(self.0);
        }
    }

    /// Reads a `CFNumber` display id out of an `IODisplay` info dictionary.
    fn display_id_number(dictionary: &CFDictionary, key_name: &'static CStr) -> Option<u32> {
        let key = CFString::from_str(key_name.to_str().expect("kDisplay*ID keys are ASCII"));
        // SAFETY: `key` is a valid CFString for the duration of the call.
        let value = unsafe { dictionary.value(NonNull::from(&*key).cast().as_ptr()) };
        let object = unsafe { value.cast::<CFType>().as_ref() }?;
        object
            .downcast_ref::<objc2_core_foundation::CFNumber>()
            .and_then(|number| number.as_i32().map(i32::cast_unsigned))
    }

    /// Finds the `IODisplayConnect` service matching the main display.
    fn main_display_service() -> Option<IoObject> {
        // SAFETY: CGMainDisplayID and the vendor/model/serial queries are
        // pure reads on CoreGraphics' display registry.
        let (vendor_id, product_id, serial) = unsafe {
            let display = objc2_core_graphics::CGMainDisplayID();
            (
                CGDisplayVendorNumber(display),
                CGDisplayModelNumber(display),
                CGDisplaySerialNumber(display),
            )
        };
        let main_port = if available!(macos = 12.0, ..) {
            // SAFETY: constant static exported by IOKit.
            unsafe { kIOMainPortDefault }
        } else {
            // SAFETY: constant static exported by IOKit.
            #[expect(
                deprecated,
                reason = "kIOMasterPortDefault is the documented entry point on macOS < 12"
            )]
            unsafe {
                kIOMasterPortDefault
            }
        };
        // SAFETY: a nul-terminated class-name literal.
        let matching = unsafe { IOServiceMatching(c"IODisplayConnect".as_ptr()) }?;
        let matching: CFRetained<CFDictionary> =
            // SAFETY: CFMutableDictionary is-a CFDictionary; ownership of the
            // +1 object is preserved through the cast.
            unsafe { CFRetained::from_raw(CFRetained::into_raw(matching).cast()) };
        let mut iterator: io_iterator_t = 0;
        // SAFETY: `matching` is consumed by the call; `iterator` is a valid
        // out-parameter receiving an io_iterator_t.
        let status =
            unsafe { IOServiceGetMatchingServices(main_port, Some(matching), &raw mut iterator) };
        if status != libc::KERN_SUCCESS {
            return None;
        }
        let iterator = IoObject(iterator);
        loop {
            let service = IOIteratorNext(iterator.0);
            if service == 0 {
                return None;
            }
            let info = IODisplayCreateInfoDictionary(service, kIODisplayOnlyPreferredName);
            let info = info.as_deref();
            let matches = info.is_some_and(|info| {
                let service_vendor = display_id_number(info, kDisplayVendorID).unwrap_or(0);
                let service_product = display_id_number(info, kDisplayProductID).unwrap_or(0);
                let service_serial = display_id_number(info, kDisplaySerialNumber).unwrap_or(0);
                let serial_matches = serial == 0 || service_serial == 0 || service_serial == serial;
                service_vendor == vendor_id && service_product == product_id && serial_matches
            });
            if matches {
                return Some(IoObject(service));
            }
            IOObjectRelease(service);
        }
    }

    pub fn get_screen_brightness() -> f32 {
        let Some(service) = main_display_service() else {
            return -1.0;
        };
        let mut brightness: f32 = -1.0;
        // SAFETY: `service` is a live IODisplay service; `brightness` is a
        // valid out pointer.
        let result = unsafe {
            IODisplayGetFloatParameter(
                service.0,
                0,
                Some(CFString::from_str(kIODisplayBrightnessKey.to_str().expect("ASCII")).as_ref()),
                &raw mut brightness,
            )
        };
        if result != libc::KERN_SUCCESS {
            return -1.0;
        }
        brightness.clamp(0.0, 1.0)
    }

    pub fn set_screen_brightness(value: f32) -> bool {
        let Some(service) = main_display_service() else {
            return false;
        };
        let clamped = value.clamp(0.0, 1.0);
        // SAFETY: `service` is a live IODisplay service.
        let result = unsafe {
            IODisplaySetFloatParameter(
                service.0,
                0,
                Some(CFString::from_str(kIODisplayBrightnessKey.to_str().expect("ASCII")).as_ref()),
                clamped,
            )
        };
        result == libc::KERN_SUCCESS
    }

    // ------------------------------------------------------------------
    // Screen stream
    // ------------------------------------------------------------------

    /// Latest captured frame metadata plus its retained `IOSurface`.
    #[derive(Clone, Debug, Default)]
    pub struct FrameSnapshot {
        pub iosurface: Option<CFRetained<IOSurfaceRef>>,
        pub sequence: u32,
        pub width: u32,
        pub height: u32,
        pub timestamp_ns: u64,
    }

    #[derive(Debug)]
    pub struct CapturerIvars {
        stream: Mutex<Option<Retained<SCStream>>>,
        running: AtomicBool,
        frame: Mutex<FrameSnapshot>,
    }

    define_class!(
        // SAFETY:
        // - The superclass NSObject does not have any subclassing requirements.
        // - `ScreenStreamCapturer` does not implement `Drop`.
        #[unsafe(super(NSObject))]
        #[thread_kind = AnyThread]
        #[name = "WaterkitScreenStreamCapturer"]
        #[ivars = CapturerIvars]
        #[derive(Debug)]
        pub struct ScreenStreamCapturer;

        unsafe impl NSObjectProtocol for ScreenStreamCapturer {}

        unsafe impl SCStreamOutput for ScreenStreamCapturer {
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
                // SAFETY: `sample_buffer` is a live frame delivered on the
                // sample handler queue.
                let Some(image_buffer) = (unsafe { sample_buffer.image_buffer() }) else {
                    return;
                };
                let (width, height, iosurface) = (
                    u32::try_from(CVPixelBufferGetWidth(&image_buffer))
                        .expect("pixel buffer width fits u32"),
                    u32::try_from(CVPixelBufferGetHeight(&image_buffer))
                        .expect("pixel buffer height fits u32"),
                    CVPixelBufferGetIOSurface(Some(&image_buffer)),
                );
                // SAFETY: `sample_buffer` is valid.
                let pts = unsafe { sample_buffer.presentation_time_stamp() };
                // SAFETY: `pts` is a well-formed CMTime.
                let timestamp_ns = unsafe { pts.seconds() } * 1_000_000_000.0;
                let mut frame = self.ivars().frame.lock().unwrap();
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "presentation timestamps are positive and fit a u64"
                )]
                {
                    frame.width = width;
                    frame.height = height;
                    frame.timestamp_ns = timestamp_ns as u64;
                }
                if let Some(surface) = iosurface {
                    frame.iosurface = Some(surface);
                    frame.sequence += 1;
                }
            }
        }

        unsafe impl SCStreamDelegate for ScreenStreamCapturer {
            #[unsafe(method(stream:didStopWithError:))]
            fn did_stop_with_error(&self, _stream: &SCStream, _error: &NSError) {
                self.ivars().running.store(false, Ordering::Relaxed);
            }
        }
    );

    impl ScreenStreamCapturer {
        fn alloc_new() -> Retained<Self> {
            let this = Self::alloc().set_ivars(CapturerIvars {
                stream: Mutex::new(None),
                running: AtomicBool::new(false),
                frame: Mutex::new(FrameSnapshot::default()),
            });
            // SAFETY: `this` is a freshly allocated capturer and `NSObject`'s
            // `init` has no additional requirements.
            unsafe { msg_send![super(this), init] }
        }

        /// Asks `SCShareableContent` for displays, builds the stream and kicks
        /// capture off; the result is delivered through `sender`.
        fn start(&self, display_id: u32, fps: u32, show_cursor: bool, sender: Sender<bool>) {
            if self.ivars().running.load(Ordering::Relaxed) {
                let _ = sender.send(true);
                return;
            }
            // The shareable-content callback holds the capturer weakly; the
            // caller's `Retained` is its only owner.
            let capturer = Weak::new(self);
            let block = RcBlock::new(
                move |content: *mut SCShareableContent, error: *mut NSError| {
                    let (Some(capturer), Some(content)) = (
                        capturer.load(),
                        if error.is_null() {
                            // SAFETY: `content` is an autoreleased object
                            // valid for the duration of the callback.
                            unsafe { content.as_ref() }
                        } else {
                            None
                        },
                    ) else {
                        let _ = sender.send(false);
                        return;
                    };
                    // SAFETY: `content` is a live SCShareableContent.
                    let displays = unsafe { content.displays() };
                    let display = displays
                        .iter()
                        .find(|display| unsafe { display.displayID() } == display_id)
                        .or_else(|| displays.iter().next());
                    let Some(display) = display else {
                        let _ = sender.send(false);
                        return;
                    };
                    let empty: Retained<NSArray<objc2_screen_capture_kit::SCWindow>> =
                        NSArray::new();
                    // SAFETY: `display` and `empty` are valid objects.
                    let filter = unsafe {
                        SCContentFilter::initWithDisplay_excludingWindows(
                            SCContentFilter::alloc(),
                            &display,
                            &empty,
                        )
                    };
                    // SAFETY: `SCStreamConfiguration` is a plain data object.
                    let config = unsafe { SCStreamConfiguration::new() };
                    // SAFETY: setters on a live configuration object; `fps`
                    // fits a CMTimeScale (Int32) like the previous code's
                    // `CMTimeScale(fps)`.
                    unsafe {
                        config.setWidth(usize::try_from(display.width()).expect("width >= 0"));
                        config.setHeight(usize::try_from(display.height()).expect("height >= 0"));
                        config.setMinimumFrameInterval(CMTime::new(
                            1,
                            i32::try_from(fps).expect("fps fits CMTimeScale"),
                        ));
                        config.setQueueDepth(8);
                        config.setPixelFormat(kCVPixelFormatType_32BGRA);
                        config.setShowsCursor(show_cursor);
                        if available!(macos = 13.0, ..) {
                            config.setCapturesAudio(false);
                        }
                    }
                    // SAFETY: `filter`/`config`/`capturer` are valid; the
                    // stream retains its configuration.
                    let stream = unsafe {
                        SCStream::initWithFilter_configuration_delegate(
                            SCStream::alloc(),
                            &filter,
                            &config,
                            Some(ProtocolObject::from_ref(&*capturer)),
                        )
                    };
                    *capturer.ivars().stream.lock().unwrap() = Some(stream.clone());
                    let queue = DispatchQueue::global_queue(
                        GlobalQueueIdentifier::QualityOfService(DispatchQoS::UserInteractive),
                    );
                    // SAFETY: `capturer` conforms to SCStreamOutput; the queue
                    // is a valid sample handler queue.
                    if unsafe {
                        stream.addStreamOutput_type_sampleHandlerQueue_error(
                            ProtocolObject::from_ref(&*capturer),
                            SCStreamOutputType::Screen,
                            Some(&queue),
                        )
                    }
                    .is_err()
                    {
                        let _ = sender.send(false);
                        return;
                    }
                    let started = {
                        let capturer = Weak::from_retained(&capturer);
                        let sender = sender.clone();
                        RcBlock::new(move |error: *mut NSError| {
                            let ok = error.is_null();
                            if ok && let Some(capturer) = capturer.load() {
                                capturer.ivars().running.store(true, Ordering::Relaxed);
                            }
                            let _ = sender.send(ok);
                        })
                    };
                    // SAFETY: `started` is a well-typed completion block.
                    unsafe { stream.startCaptureWithCompletionHandler(Some(&started)) };
                },
            );
            // SAFETY: `block` is a well-typed completion block.
            unsafe {
                SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
                    false,
                    true,
                    &block,
                );
            };
        }

        /// Latest frame state, or nothing new yet.
        pub fn frame_snapshot(&self) -> FrameSnapshot {
            self.ivars().frame.lock().unwrap().clone()
        }

        /// Stops the capture and clears the last frame, like the previous
        /// implementation's `stop` + state reset.
        pub fn stop(&self) {
            if self.ivars().running.swap(false, Ordering::Relaxed) {
                let stream = self.ivars().stream.lock().unwrap().clone();
                if let Some(stream) = stream {
                    // SAFETY: stopping a running stream with no completion is
                    // always valid.
                    unsafe { stream.stopCaptureWithCompletionHandler(None) };
                }
            }
            *self.ivars().frame.lock().unwrap() = FrameSnapshot::default();
        }
    }

    /// Initialize the screen stream and return the capturer on success.
    pub fn init_screen_stream(
        display_id: u32,
        target_fps: u32,
        show_cursor: bool,
    ) -> Option<Retained<ScreenStreamCapturer>> {
        if !available!(macos = 12.3, ..) {
            return None;
        }
        let capturer = ScreenStreamCapturer::alloc_new();
        let (sender, receiver) = mpsc::channel();
        capturer.start(display_id, target_fps, show_cursor, sender);
        let success = wait(&receiver, Duration::from_secs(3)).unwrap_or(false);
        success.then_some(capturer)
    }
}

// ============================================================================
// Screenshot
// ============================================================================

/// Capture a screenshot with the specified format.
pub fn screenshot(display: &ScreenInfo, format: ImageFormat) -> Result<Screenshot, Error> {
    let format_code = match format {
        ImageFormat::Png => 0,
        ImageFormat::Avif => 1,
        ImageFormat::Heif => 2,
    };

    #[cfg(target_os = "ios")]
    let data = ios::capture_screenshot(format_code);
    #[cfg(target_os = "macos")]
    let data = macos::capture_screenshot(format_code);

    if data.is_empty() {
        return Err(Error::Platform("Screenshot capture failed".into()));
    }

    Ok(Screenshot::new(
        data,
        display.width(),
        display.height(),
        format,
    ))
}

// ============================================================================
// Screens (iOS only - macOS uses desktop module)
// ============================================================================

#[cfg(target_os = "ios")]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the public API returns a Result on every platform"
)]
pub fn screens() -> Result<Vec<ScreenInfo>, Error> {
    // iOS has a single main screen conceptually
    Ok(vec![ScreenInfo::new(
        0,
        "Main Screen".into(),
        0, // Would need UIScreen.main.bounds
        0,
        1.0,
        true,
    )])
}

// ============================================================================
// Brightness
// ============================================================================

#[cfg(target_os = "ios")]
#[expect(
    clippy::unused_async,
    reason = "the public API is async on every platform"
)]
pub async fn get_brightness() -> Result<f32, Error> {
    let value = ios::get_screen_brightness();
    if !(0.0..=1.0).contains(&value) {
        return Err(Error::Platform(format!(
            "invalid iOS brightness value from platform bridge: {value}"
        )));
    }

    Ok(value)
}

#[cfg(target_os = "ios")]
#[expect(
    clippy::unused_async,
    reason = "the public API is async on every platform"
)]
pub async fn set_brightness(val: f32) -> Result<(), Error> {
    if ios::set_screen_brightness(val.clamp(0.0, 1.0)) {
        Ok(())
    } else {
        Err(Error::Platform(
            "failed to set iOS screen brightness".into(),
        ))
    }
}

#[cfg(target_os = "macos")]
pub fn get_macos_brightness() -> Result<f32, Error> {
    let value = macos::get_screen_brightness();
    if !(0.0..=1.0).contains(&value) {
        return Err(Error::Platform(format!(
            "invalid macOS brightness value from platform bridge: {value}"
        )));
    }

    Ok(value)
}

#[cfg(target_os = "macos")]
pub fn set_macos_brightness(value: f32) -> Result<(), Error> {
    if macos::set_screen_brightness(value.clamp(0.0, 1.0)) {
        Ok(())
    } else {
        Err(Error::Platform(
            "failed to set macOS screen brightness".into(),
        ))
    }
}

// ============================================================================
// ScreenStreamInner
// ============================================================================

/// Screen stream with IOSurface-based capture on macOS.
pub struct ScreenStreamInner {
    width: u32,
    height: u32,
    #[cfg(target_os = "macos")]
    capturer: objc2::rc::Retained<macos::ScreenStreamCapturer>,
    #[cfg(target_os = "macos")]
    last_sequence: std::sync::atomic::AtomicU32,
    #[cfg(target_os = "macos")]
    device: Arc<Device>,
    #[cfg(target_os = "macos")]
    queue: Arc<Queue>,
}

impl ScreenStreamInner {
    /// Create a new screen stream.
    #[cfg(target_os = "macos")]
    pub fn new(
        display: &ScreenInfo,
        device: Arc<Device>,
        queue: Arc<Queue>,
        config: &StreamConfig,
    ) -> Result<Self, Error> {
        let Some(capturer) =
            macos::init_screen_stream(display.id(), config.target_fps, config.show_cursor)
        else {
            return Err(Error::Platform("Failed to initialize screen stream".into()));
        };

        Ok(Self {
            width: display.width(),
            height: display.height(),
            capturer,
            last_sequence: std::sync::atomic::AtomicU32::new(0),
            device,
            queue,
        })
    }

    /// Create a new screen stream (iOS - unsupported).
    #[cfg(target_os = "ios")]
    #[expect(
        clippy::unnecessary_wraps,
        reason = "the public API returns a Result on every platform"
    )]
    pub fn new(
        display: &ScreenInfo,
        _device: Arc<Device>,
        _queue: Arc<Queue>,
        _config: &StreamConfig,
    ) -> Result<Self, Error> {
        Ok(Self {
            width: display.width(),
            height: display.height(),
        })
    }

    /// Get next frame asynchronously.
    #[expect(
        clippy::unused_async,
        reason = "the public API is async on every platform"
    )]
    #[cfg_attr(
        target_os = "macos",
        expect(
            clippy::future_not_send,
            reason = "the capturer's Objective-C ivars are not Sync, so `&self` futures are not Send; the public ScreenStream wrapper already expects this"
        )
    )]
    pub async fn next_frame(&self) -> Option<ScreenFrame> {
        self.try_next_frame()
    }

    /// Try to get a frame without blocking.
    #[cfg(target_os = "macos")]
    pub fn try_next_frame(&self) -> Option<ScreenFrame> {
        use std::sync::atomic::Ordering;

        let frame = self.capturer.frame_snapshot();
        // No new frame
        if frame.sequence == self.last_sequence.load(Ordering::Relaxed) || frame.sequence == 0 {
            return None;
        }
        let iosurface = frame.iosurface?;
        let (sequence, width, height, timestamp_ns) = (
            frame.sequence,
            frame.width,
            frame.height,
            frame.timestamp_ns,
        );

        // Read BGRA data from IOSurface and upload to GPU
        let bgra_data = read_iosurface_bgra(&iosurface, width, height);

        self.last_sequence.store(sequence, Ordering::Relaxed);

        // Create GPU texture
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ScreenCapture"),
            size: Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Bgra8UnormSrgb,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Upload data
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &bgra_data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        Some(ScreenFrame::from_texture(
            Arc::new(texture),
            width,
            height,
            TextureFormat::Bgra8UnormSrgb,
            timestamp_ns,
        ))
    }

    #[cfg(target_os = "ios")]
    #[expect(clippy::unused_self, reason = "iOS has no screen stream to read from")]
    pub const fn try_next_frame(&self) -> Option<ScreenFrame> {
        // iOS doesn't support screen capture streaming
        None
    }

    /// Get capture dimensions.
    pub const fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

#[cfg(target_os = "macos")]
impl Drop for ScreenStreamInner {
    fn drop(&mut self) {
        self.capturer.stop();
    }
}

// ============================================================================
// IOSurface BGRA reading (macOS only)
// ============================================================================

#[cfg(target_os = "macos")]
fn read_iosurface_bgra(
    surface: &objc2_io_surface::IOSurfaceRef,
    width: u32,
    height: u32,
) -> Vec<u8> {
    use objc2_io_surface::IOSurfaceLockOptions;
    use std::ptr;

    let expected_size = (width * height * 4) as usize;
    let mut bgra_data = vec![0u8; expected_size];

    unsafe {
        // Lock for reading
        surface.lock(IOSurfaceLockOptions::ReadOnly, ptr::null_mut());

        // Get base address and stride
        let base_address = surface.base_address();
        let bytes_per_row = surface.bytes_per_row();
        let src = base_address.as_ptr().cast::<u8>();
        let row_bytes = (width * 4) as usize;

        // Copy row by row (handle stride)
        if bytes_per_row == row_bytes {
            // Fast path: no stride padding
            ptr::copy_nonoverlapping(src, bgra_data.as_mut_ptr(), expected_size);
        } else {
            // Handle stride
            for row in 0..height as usize {
                ptr::copy_nonoverlapping(
                    src.add(row * bytes_per_row),
                    bgra_data.as_mut_ptr().add(row * row_bytes),
                    row_bytes,
                );
            }
        }

        surface.unlock(IOSurfaceLockOptions::ReadOnly, ptr::null_mut());
    }

    bgra_data
}
