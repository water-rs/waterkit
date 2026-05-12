//! Shared error variants for cross-cutting failures.
//!
//! Each capability crate defines its own `Error` enum, but several variants
//! are universal: "this platform does not support the operation",
//! "permission was denied", "platform-specific failure with a message".
//! Crates may embed [`CoreError`] inside their own enum or simply mirror
//! its variants — using the same shape makes downstream `match` patterns
//! consistent.

use thiserror::Error;

/// Cross-cutting error variants shared by waterkit capability crates.
#[derive(Debug, Clone, Error)]
#[non_exhaustive]
pub enum CoreError {
    /// Operation is not supported on the current platform / OS / device.
    #[error("operation not supported on this platform")]
    Unsupported,

    /// User has not granted the required permission, or the OS denies it
    /// at policy level (e.g. parental controls, MDM).
    #[error("permission denied")]
    PermissionDenied,

    /// Platform-level failure with a message from the underlying API.
    #[error("platform error: {0}")]
    Platform(String),
}
