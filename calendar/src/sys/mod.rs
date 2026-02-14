//! Platform-specific calendar implementations.

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use apple::*;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub use android::*;

#[cfg(any(target_os = "windows", target_os = "linux"))]
mod desktop;
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub use desktop::*;

#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
mod fallback;
#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
pub use fallback::*;
