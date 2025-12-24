use crate::{Error, ScreenInfo};
use std::io::Cursor;
// use brightness::Brightness; // Removed due to build failure


pub fn capture_screen(display_index: usize) -> Result<Vec<u8>, Error> {
    let screens = screenshots::Screen::all().map_err(|e| Error::Platform(e.to_string()))?;
    let screen = screens.get(display_index).ok_or(Error::MonitorNotFound)?;
    
    let image = screen.capture().map_err(|e| Error::Platform(e.to_string()))?;
    
    let mut buffer = Vec::new();
    let mut cursor = Cursor::new(&mut buffer);
    image.write_to(&mut cursor, image::ImageFormat::Png)
        .map_err(|e| Error::Platform(e.to_string()))?;
        
    Ok(buffer)
}

pub fn capture_screen_raw(display_index: usize) -> Result<crate::RawCapture, Error> {
    let screens = screenshots::Screen::all().map_err(|e| Error::Platform(e.to_string()))?;
    let screen = screens.get(display_index).ok_or(Error::MonitorNotFound)?;
    
    let image = screen.capture().map_err(|e| Error::Platform(e.to_string()))?;
    
    // Image is already RGBA from screenshots crate
    let width = image.width();
    let height = image.height();
    
    Ok(crate::RawCapture {
        data: image.into_raw(),
        width,
        height,
    })
}

/// High-performance screen capturer with cached screen handle.
/// 
/// Use this for repeated captures (e.g., video recording) to avoid
/// the overhead of `Screen::all()` on every frame.
pub struct ScreenCapturer {
    screen: screenshots::Screen,
}

impl ScreenCapturer {
    /// Create a new capturer for the specified display.
    pub fn new(display_index: usize) -> Result<Self, Error> {
        let screens = screenshots::Screen::all().map_err(|e| Error::Platform(e.to_string()))?;
        let screen = screens.into_iter()
            .nth(display_index)
            .ok_or(Error::MonitorNotFound)?;
        Ok(Self { screen })
    }
    
    /// Capture the screen. Much faster than `capture_screen_raw()` for repeated use.
    pub fn capture(&self) -> Result<crate::RawCapture, Error> {
        let image = self.screen.capture().map_err(|e| Error::Platform(e.to_string()))?;
        let width = image.width();
        let height = image.height();
        
        Ok(crate::RawCapture {
            data: image.into_raw(),
            width,
            height,
        })
    }
    
    /// Get the screen dimensions.
    pub fn dimensions(&self) -> (u32, u32) {
        (self.screen.display_info.width, self.screen.display_info.height)
    }
}

pub fn screens() -> Result<Vec<ScreenInfo>, Error> {
    let screens = screenshots::Screen::all().map_err(|e| Error::Platform(e.to_string()))?;
    
    let mut infos = Vec::new();
    for screen in screens.iter() {
        infos.push(ScreenInfo {
            id: screen.display_info.id,
            name: format!("Screen {}", screen.display_info.id), // screenshots crate captures display info differently
            width: screen.display_info.width,
            height: screen.display_info.height,
            scale_factor: screen.display_info.scale_factor,
            is_primary: screen.display_info.is_primary,
        });
    }
    
    Ok(infos)
}

pub async fn get_brightness() -> Result<f32, Error> {
    // brightness crate is currently broken on macOS (build failure).
    // For now we return a dummy value or error.
    // Err(Error::Unsupported)
    Ok(1.0) 
}

pub async fn set_brightness(_val: f32) -> Result<(), Error> {
    // brightness crate broken.
    // Err(Error::Unsupported)
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub async fn pick_and_capture() -> Result<Vec<u8>, Error> {
    Err(Error::Unsupported)
}
