//! Cross-platform haptic feedback.
//!
//! This crate provides a unified API for triggering haptic feedback (vibration)
//! across iOS, macOS, Android, Windows, and Linux platforms.

#![warn(missing_docs)]

// Internal platform-specific implementations.
mod sys;

/// Types of haptic feedback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HapticFeedback {
    /// A lightweight impact, like a key press.
    Light,
    /// A medium impact.
    Medium,
    /// A heavy impact.
    Heavy,
    /// A rigid, sharp impact.
    Rigid,
    /// A soft, dull impact.
    Soft,
    /// A feedback indicating a selection change (e.g., picker wheel).
    Selection,
    /// A notification indicating success.
    Success,
    /// A notification indicating a warning.
    Warning,
    /// A notification indicating an error.
    Error,
}

/// Errors that can occur when triggering haptic feedback.
#[derive(Debug, Clone, thiserror::Error)]
pub enum HapticError {
    /// Haptic feedback is not supported on this device.
    #[error("haptic feedback not supported")]
    NotSupported,
    /// An unknown error occurred.
    #[error("unknown error: {0}")]
    Unknown(String),
}

/// Trigger haptic feedback.
///
/// This function triggers the specified type of haptic feedback on the device.
///
/// # Errors
/// Returns an error if the haptic feedback is not supported or fails to trigger.
pub async fn feedback(style: HapticFeedback) -> Result<(), HapticError> {
    sys::feedback(style).await
}
