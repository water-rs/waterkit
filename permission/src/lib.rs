//! Cross-platform permission request handling.
//!
//! This crate provides a unified API for requesting permissions across
//! iOS, macOS, Android, Windows, and Linux platforms.

#![warn(missing_docs)]

/// Platform-specific implementations.
pub mod sys;

/// Types of permissions that can be requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Permission {
    /// Access to device location.
    Location,
    /// Access to device camera.
    Camera,
    /// Access to device microphone.
    Microphone,
    /// Access to photo library.
    Photos,
    /// Access to contacts.
    Contacts,
    /// Access to calendar.
    Calendar,
}

/// The current status of a permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PermissionStatus {
    /// Permission has been granted by the user.
    Granted,
    /// Permission has been denied by the user.
    Denied,
    /// Permission is restricted (e.g., parental controls on iOS).
    Restricted,
    /// Permission has not been requested yet.
    NotDetermined,
}

/// Errors that can occur when requesting permissions.
#[derive(Debug, Clone)]
pub enum PermissionError {
    /// The permission type is not supported on this platform.
    NotSupported,
    /// An unknown error occurred.
    Unknown(String),
}

impl std::fmt::Display for PermissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSupported => write!(f, "permission not supported on this platform"),
            Self::Unknown(msg) => write!(f, "unknown error: {msg}"),
        }
    }
}

impl std::error::Error for PermissionError {}

/// Check the current status of a permission without requesting it.
pub async fn check(permission: Permission) -> PermissionStatus {
    sys::check(permission).await
}

/// Request a permission from the user.
///
/// If the permission has already been granted or denied, this returns
/// the current status without showing a prompt.
///
/// # Errors
/// Returns a `PermissionError` if:
/// - The permission type is not supported on this platform.
/// - An underlying platform error occurs.
pub async fn request(permission: Permission) -> Result<PermissionStatus, PermissionError> {
    sys::request(permission).await
}
