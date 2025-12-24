use crate::{Error, ScreenInfo};

#[swift_bridge::bridge]
mod ffi {
    extern "Rust" {
        // Rust types exposed to Swift
        fn on_picker_result(data: Vec<u8>);
    }

    extern "Swift" {
        // Swift function declarations
        fn get_screen_brightness() -> f32;
        fn set_screen_brightness(value: f32);
        
        // Return PNG bytes (iOS snapshot)
        fn capture_main_screen() -> Vec<u8>; 
        
        // macOS picker
        fn show_picker_and_capture();
        
        // High-speed ScreenCaptureKit streaming (macOS 12.3+)
        fn init_sck_stream() -> bool;
        fn stop_sck_stream();
        fn get_latest_frame() -> Vec<u8>;
        fn get_frame_count() -> u32;
        fn reset_frame_count();
        
        // Zero-copy IOSurface access
        fn get_iosurface_ptr() -> u64;
        fn get_iosurface_sequence() -> u32;

        // Control raw frame copying (disable for zero-copy pipelines)
        fn set_raw_frame_capture_enabled(enabled: bool);
    }
}

// Global callback channel for picker
// Since swift-bridge doesn't support async easy, we use a global channel/callback mechanism.
use std::sync::Mutex;
use std::sync::OnceLock;
use tokio::sync::oneshot;

static PICKER_SENDER: OnceLock<Mutex<Option<oneshot::Sender<Option<Vec<u8>>>>>> = OnceLock::new();

fn on_picker_result(data: Vec<u8>) {
    if let Some(mutex) = PICKER_SENDER.get() {
        if let Ok(mut lock) = mutex.lock() {
            if let Some(sender) = lock.take() {
                let res = if data.is_empty() { None } else { Some(data) };
                let _ = sender.send(res);
            }
        }
    }
}


#[cfg(target_os = "ios")]
pub fn capture_screen(display_index: usize) -> Result<Vec<u8>, Error> {
    // Mobile only has one "main" screen usually for apps.
    if display_index != 0 {
        return Err(Error::MonitorNotFound);
    }
    
    let bytes = ffi::capture_main_screen();
    if bytes.is_empty() {
        Err(Error::Platform("Failed to capture screen".into()))
    } else {
        Ok(bytes)
    }
}

#[cfg(target_os = "ios")]
pub async fn get_brightness() -> Result<f32, Error> {
    Ok(ffi::get_screen_brightness())
}

#[cfg(target_os = "ios")]
pub async fn set_brightness(val: f32) -> Result<(), Error> {
    ffi::set_screen_brightness(val);
    Ok(())
}

#[cfg(target_os = "ios")]
pub fn screens() -> Result<Vec<ScreenInfo>, Error> {
    // Helper to get screen size (not implemented in bridge yet)
    // Placeholder
    Ok(vec![ScreenInfo {
        id: 0,
        name: "Main Screen".into(),
        width: 0, 
        height: 0,
        scale_factor: 1.0,
        is_primary: true,
    }])
}

#[cfg(target_os = "ios")]
pub async fn pick_and_capture() -> Result<Vec<u8>, Error> {
    // Can implement for iOS later if needed
    Err(Error::Unsupported)
}

#[cfg(target_os = "macos")]
pub async fn pick_and_capture() -> Result<Vec<u8>, Error> {
    let (tx, rx) = oneshot::channel();
    
    {
        let mutex = PICKER_SENDER.get_or_init(|| Mutex::new(None));
        let mut lock = mutex.lock().map_err(|_| Error::Platform("Lock error".into()))?;
        *lock = Some(tx);
    }
    
    ffi::show_picker_and_capture();
    
    match rx.await {
        Ok(Some(bytes)) => Ok(bytes),
        Ok(None) => Err(Error::Platform("Picker cancelled or failed".into())),
        Err(_) => Err(Error::Platform("Picker channel closed".into())),
    }
}

/// High-speed ScreenCaptureKit-based screen capturer (macOS 12.3+).
/// 
/// Uses SCStream for 60fps+ capable frame capture.
/// Much faster than CGWindowListCreateImage-based approaches.
#[cfg(target_os = "macos")]
pub struct SCKCapturer {
    _private: (),
}

#[cfg(target_os = "macos")]
impl SCKCapturer {
    /// Initialize the ScreenCaptureKit stream.
    /// Returns None if SCK is not available (macOS < 12.3).
    pub fn new() -> Option<Self> {
        if ffi::init_sck_stream() {
            Some(Self { _private: () })
        } else {
            None
        }
    }

    /// Initialize the ScreenCaptureKit stream with a descriptive error.
    pub fn try_new() -> Result<Self, Error> {
        if ffi::init_sck_stream() {
            Ok(Self { _private: () })
        } else {
            Err(Error::Platform("Failed to initialize ScreenCaptureKit".into()))
        }
    }
    
    /// Get the latest captured frame as raw BGRA bytes.
    /// Returns (width, height, data) or None if no frame available yet.
    pub fn get_frame(&self) -> Option<crate::RawCapture> {
        let data = ffi::get_latest_frame();
        if data.len() < 8 {
            return None;
        }
        
        // Decode width and height from first 8 bytes
        let width = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let height = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        
        // Check if this is dimensions-only response (9th byte = 0xFF)
        if data.len() == 9 && data[8] == 0xFF {
            // SCK stream is running, return dummy frame with dimensions
            Some(crate::RawCapture {
                data: vec![], // Empty for timing test
                width,
                height,
            })
        } else if data.len() == 8 + (width * height * 4) as usize {
            Some(crate::RawCapture {
                data: data[8..].to_vec(),
                width,
                height,
            })
        } else {
            None
        }
    }

    /// Get the latest frame if pixel data is available (skips dimension-only replies).
    pub fn latest_frame(&self) -> Option<crate::RawCapture> {
        let frame = self.get_frame()?;
        if frame.data.is_empty() {
            None
        } else {
            Some(frame)
        }
    }

    /// Get the most recently reported frame dimensions.
    pub fn dimensions(&self) -> Option<(u32, u32)> {
        self.get_frame().map(|frame| (frame.width, frame.height))
    }
    
    /// Get the number of unique frames captured by ScreenCaptureKit.
    pub fn frame_count(&self) -> u32 {
        ffi::get_frame_count()
    }
    
    /// Reset the frame counter.
    pub fn reset_frame_count(&self) {
        ffi::reset_frame_count();
    }
    
    /// Get the raw IOSurface pointer for zero-copy GPU access.
    /// Returns None if no IOSurface is available.
    pub fn iosurface_ptr(&self) -> Option<u64> {
        let ptr = ffi::get_iosurface_ptr();
        if ptr == 0 { None } else { Some(ptr) }
    }
    
    /// Get the IOSurface sequence number to detect new frames.
    pub fn iosurface_sequence(&self) -> u32 {
        ffi::get_iosurface_sequence()
    }

    /// Enable or disable raw frame copy to CPU memory.
    pub fn set_raw_frames_enabled(&self, enabled: bool) {
        ffi::set_raw_frame_capture_enabled(enabled);
    }
}

#[cfg(target_os = "macos")]
impl Drop for SCKCapturer {
    fn drop(&mut self) {
        ffi::stop_sck_stream();
    }
}
