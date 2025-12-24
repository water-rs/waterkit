#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
mod desktop;
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub use desktop::*;

#[cfg(any(target_os = "ios"))]
mod apple;
#[cfg(any(target_os = "ios"))]
pub use apple::*;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub use android::*;

// Fallback for docs or other platforms
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux", target_os = "ios", target_os = "android")))]
mod dummy {
    use crate::{Error, ScreenInfo};
    pub fn capture_screen(_idx: usize) -> Result<Vec<u8>, Error> { Err(Error::Unsupported) }
    pub async fn get_brightness() -> Result<f32, Error> { Err(Error::Unsupported) }
    pub async fn set_brightness(_val: f32) -> Result<(), Error> { Err(Error::Unsupported) }
    pub fn screens() -> Result<Vec<ScreenInfo>, Error> { Err(Error::Unsupported) }
}
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux", target_os = "ios", target_os = "android")))]
pub use dummy::*;
