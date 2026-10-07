//! Platform-specific native realizations of the text request.

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use apple::*;

#[cfg(not(any(target_os = "windows", target_os = "ios", target_os = "macos")))]
mod unsupported;
#[cfg(not(any(target_os = "windows", target_os = "ios", target_os = "macos")))]
pub use unsupported::*;
