//! Notification error types.

/// Errors that can occur when showing notifications.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum NotificationError {
    /// The notification service is unavailable.
    #[error("notification service unavailable")]
    ServiceUnavailable,

    /// Permission to show notifications was denied.
    #[error("permission denied")]
    PermissionDenied,

    /// The notification asked for something the platform cannot show.
    ///
    /// The message names the input that is not supported.
    #[error("unsupported notification input: {0}")]
    Unsupported(String),

    /// A platform-specific error occurred.
    #[error("platform error: {0}")]
    Platform(String),
}
