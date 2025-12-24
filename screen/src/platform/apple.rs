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
