//! Clipboard error types.

/// Errors that can occur during clipboard operations.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum ClipboardError {
    /// The clipboard is not available on this platform or context.
    #[error("clipboard unavailable")]
    Unavailable,

    /// Permission to access the clipboard was denied.
    ///
    /// This can occur on:
    /// - Android 10+: app doesn't have focus or lacks `READ_CLIPBOARD` permission
    /// - iOS 14+: user denied clipboard access
    /// - macOS: sandboxed app without clipboard entitlement
    #[error("permission denied")]
    PermissionDenied,

    /// The requested content type is not supported on this platform.
    #[error("unsupported content type: {0}")]
    UnsupportedType(String),

    /// Invalid image data was provided or received.
    #[error("invalid image data: {0}")]
    InvalidImage(String),

    /// Failed to encode data for clipboard.
    #[error("encode error: {0}")]
    Encode(String),

    /// Failed to decode data from clipboard.
    #[error("decode error: {0}")]
    Decode(String),

    /// A platform-specific error occurred.
    #[error("platform error: {0}")]
    Platform(String),
}
