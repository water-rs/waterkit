//! Cross-platform haptic feedback.
//!
//! This crate provides a unified API for triggering haptic feedback (vibration)
//! across iOS, macOS, Android, Windows, and Linux platforms.

#![warn(missing_docs)]

/// Platform-specific implementations.
pub mod sys;

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
#[derive(Debug, Clone)]
pub enum HapticError {
    /// Haptic feedback is not supported on this device.
    NotSupported,
    /// An unknown error occurred.
    Unknown(String),
}

impl std::fmt::Display for HapticError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSupported => write!(f, "haptic feedback not supported"),
            Self::Unknown(msg) => write!(f, "unknown error: {msg}"),
        }
    }
}

impl std::error::Error for HapticError {}

/// Trigger haptic feedback.
///
/// This function triggers the specified type of haptic feedback on the device.
pub async fn feedback(style: HapticFeedback) -> Result<(), HapticError> {
    sys::feedback(style).await
}
