//! Passkey error definitions.

/// Errors returned by passkey operations.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum PasskeyError {
    /// Passkeys are unsupported on this platform.
    #[error("passkeys are not supported on this platform")]
    NotSupported,
    /// Passkeys are supported but unavailable (for example not configured by the user).
    #[error("passkeys are not available on this device")]
    NotAvailable,
    /// User cancelled the in-progress ceremony.
    #[error("passkey operation was cancelled")]
    Cancelled,
    /// Input validation failed.
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// Platform backend returned an error.
    #[error("platform error: {0}")]
    Platform(String),
    /// Operation failed for a non-platform-specific reason.
    #[error("operation failed: {0}")]
    OperationFailed(String),
}

impl PasskeyError {
    /// Maps a raw platform error string into a typed passkey error.
    #[must_use]
    pub fn from_platform_error(message: impl Into<String>) -> Self {
        let message = message.into();
        let lowered = message.to_ascii_lowercase();
        if lowered.contains("cancel") || lowered.contains("abort") || lowered.contains("dismiss") {
            return Self::Cancelled;
        }

        if lowered.contains("not supported") {
            return Self::NotSupported;
        }

        if lowered.contains("not available")
            || lowered.contains("unavailable")
            || lowered.contains("not configured")
        {
            return Self::NotAvailable;
        }

        Self::Platform(message)
    }
}
