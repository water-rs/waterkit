//! Notification error types.

/// Errors that can occur when showing notifications.
#[derive(Debug, Clone, thiserror::Error)]
pub enum NotificationError {
    /// The notification service is unavailable.
    #[error("notification service unavailable")]
    ServiceUnavailable,

    /// Permission to show notifications was denied.
    #[error("permission denied")]
    PermissionDenied,

    /// A platform-specific error occurred.
    #[error("platform error: {0}")]
    Platform(String),
}
