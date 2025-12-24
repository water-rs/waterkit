use crate::{Error, ScreenInfo};

#[swift_bridge::bridge]
mod ffi {
    extern "Rust" {
        // Rust types exposed to Swift
    }

    extern "Swift" {
        // Swift function declarations
        fn get_screen_brightness() -> f32;
        fn set_screen_brightness(value: f32);
        
        // Return PNG bytes
        fn capture_main_screen() -> Option<Vec<u8>>; 
        
        // For list of screens, iOS usually has 1 (or external). macOS has many.
        // We'll return basic info for standard screen.
        // But swift-bridge doesn't support returning complex structs easily without defines.
        // For now, let's implement brightness and capture.
    }
}

pub fn capture_screen(display_index: usize) -> Result<Vec<u8>, Error> {
    // Mobile only has one "main" screen usually for apps.
    if display_index != 0 {
        return Err(Error::MonitorNotFound);
    }
    
    if let Some(bytes) = ffi::capture_main_screen() {
        Ok(bytes)
    } else {
        Err(Error::Platform("Failed to capture screen".into()))
    }
}

pub async fn get_brightness() -> Result<f32, Error> {
    Ok(ffi::get_screen_brightness())
}

pub async fn set_brightness(val: f32) -> Result<(), Error> {
    ffi::set_screen_brightness(val);
    Ok(())
}

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
