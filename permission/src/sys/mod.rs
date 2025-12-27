//! Platform-specific permission implementations.

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
// Re-export platform implementations
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use apple::{check, request};

#[cfg(target_os = "android")]
pub use android::{check, request};

#[cfg(target_os = "windows")]
pub use windows::{check, request};

#[cfg(target_os = "linux")]
pub use linux::{check, request};

// Fallback for unsupported platforms (compile-time stub)
#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
pub(crate) async fn check(_permission: crate::Permission) -> crate::PermissionStatus {
    crate::PermissionStatus::NotDetermined
}

#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
pub(crate) async fn request(
    _permission: crate::Permission,
) -> Result<crate::PermissionStatus, crate::PermissionError> {
    Err(crate::PermissionError::NotSupported)
}
