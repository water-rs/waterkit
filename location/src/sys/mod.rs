//! Platform-specific location implementations.

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;

/// Android platform implementation.
#[cfg(target_os = "android")]
pub mod android;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
mod linux;

// Re-export platform implementations
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub(crate) use apple::get_location;

#[cfg(target_os = "android")]
pub(crate) use android::get_location;

#[cfg(target_os = "windows")]
pub(crate) use windows::get_location;

#[cfg(target_os = "linux")]
pub(crate) use linux::get_location;

// Fallback for unsupported platforms
#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
pub(crate) async fn get_location() -> Result<crate::Location, crate::LocationError> {
    Err(crate::LocationError::NotAvailable)
}
