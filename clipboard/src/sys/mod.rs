//! Platform-specific clipboard backend implementations.

#[cfg(any(target_os = "windows", target_os = "linux"))]
/// Desktop platform backend.
pub mod desktop;
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub use desktop::*;

#[cfg(target_os = "android")]
/// Android platform backend.
pub mod android;
#[cfg(target_os = "android")]
pub use android::*;

#[cfg(any(target_os = "ios", target_os = "macos"))]
/// Apple platform backend.
pub mod apple;
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use apple::*;
