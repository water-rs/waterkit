#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
mod desktop;
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub use desktop::*;

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use apple::*;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub use android::*;

// Fallback / Dummy impls

// For platform that don't have pick_and_capture (windows, linux, ios, android)
// Windows/Linux use desktop, which doesn't have it yet.
// iOS uses apple, which doesn't have it yet (only macos).
// Android doesn't have it.
// We need to define stubs.

// Actually desktop.rs (Linux/Windows/MacOS) handles capture, screens, brightness (stub).
// desktop.rs needs `pick_and_capture` stub for Linux/Windows?
// Or we implement it in desktop.rs as unsupported?
// Yes, easiest is to add `pick_and_capture` to desktop.rs (returning Unsupported)
// AND implementation in `apple.rs` for MacOS.
// But we have conflict if both export `pick_and_capture` on macOS.
// `desktop` is `any(macos, windows, linux)`.
// `apple` is `any(ios, macos)`.
// So on macOS, both are present.
// We should remove `pick_and_capture` from `desktop.rs` on macOS?
// Or make `desktop.rs` NOT generic for macOS if we are moving to `apple.rs`?
// No, `capture_screen` is in `desktop.rs` for macOS.

// Solution: `pick_and_capture` in `desktop.rs` should conform to `cfg(not(target_os = "macos"))`.
// And `pick_and_capture` in `apple.rs` should conform to `cfg(target_os = "macos")`.
// iOS also needs `pick_and_capture`.

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux", target_os = "ios", target_os = "android")))]
mod dummy {
    use crate::{Error, ScreenInfo};
    pub fn capture_screen(_idx: usize) -> Result<Vec<u8>, Error> { Err(Error::Unsupported) }
    pub async fn pick_and_capture() -> Result<Vec<u8>, Error> { Err(Error::Unsupported) }
    pub async fn get_brightness() -> Result<f32, Error> { Err(Error::Unsupported) }
    pub async fn set_brightness(_val: f32) -> Result<(), Error> { Err(Error::Unsupported) }
    pub fn screens() -> Result<Vec<ScreenInfo>, Error> { Err(Error::Unsupported) }
}
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux", target_os = "ios", target_os = "android")))]
pub use dummy::*;
