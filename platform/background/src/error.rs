//! Error types for background task APIs.

/// Errors produced by background scheduling and execution APIs.
#[derive(Debug, Clone, thiserror::Error)]
pub enum BackgroundError {
    /// This platform does not support background tasks via this module.
    #[error("background tasks are not supported on this platform")]
    Unsupported,

    /// The provided identifier was invalid.
    #[error("invalid task identifier `{identifier}`: {reason}")]
    InvalidIdentifier {
        /// The invalid identifier value.
        identifier: String,
        /// A human-readable validation failure reason.
        reason: String,
    },

    /// The provided continued-processing pattern was invalid.
    #[error("invalid continued task pattern `{pattern}`: {reason}")]
    InvalidContinuedPattern {
        /// The invalid pattern.
        pattern: String,
        /// A human-readable validation failure reason.
        reason: String,
    },

    /// Runtime initialization happened too late for platform requirements.
    #[error("background runtime initialization failed: {0}")]
    LateInitialization(String),

    /// A task was submitted without being registered during bootstrap.
    #[error("task identifier `{identifier}` is not registered for {kind}")]
    HandlerNotRegistered {
        /// Unregistered identifier.
        identifier: String,
        /// Task kind string.
        kind: &'static str,
    },

    /// Required platform configuration was missing.
    #[error("background configuration missing: {0}")]
    ConfigurationMissing(String),

    /// Scheduler rejected a task submission.
    #[error("scheduler rejected request ({code}): {message}")]
    SchedulerRejected {
        /// Platform-specific error code.
        code: i32,
        /// Platform-specific rejection reason.
        message: String,
    },

    /// A task token was invalid or already completed.
    #[error("invalid task token: {0}")]
    InvalidTaskToken(String),

    /// A task-kind specific operation was used with the wrong kind.
    #[error("task operation is not valid for {0}")]
    InvalidTaskKind(&'static str),

    /// Generic platform-specific failure.
    #[error("platform error: {0}")]
    Platform(String),
}
