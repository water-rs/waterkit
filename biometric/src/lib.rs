//! This crate provides a unified API for biometric authentication (`TouchID`, `FaceID`, fingerprint, etc.)
//! across iOS, macOS, Android, and Windows.

#![warn(missing_docs)]

/// Platform-specific implementations.
mod sys;

use thiserror::Error;

/// The type of biometric authentication available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BiometricType {
    /// Fingerprint authentication (`TouchID`, Android fingerprint, etc.)
    Fingerprint,
    /// Facial recognition (`FaceID`, Android Face Unlock, Windows Hello Face)
    Face,
    /// Iris scanning
    Iris,
    /// Unknown or other biometric type
    Unknown,
}

/// Errors that can occur during biometric authentication.
#[derive(Debug, Error)]
pub enum BiometricError {
    /// Biometric authentication is not available on this device.
    #[error("Biometric authentication is not available on this device")]
    NotAvailable,
    /// User cancelled the authentication.
    #[error("User cancelled the authentication")]
    Cancelled,
    /// Authentication failed with a specific message.
    #[error("Authentication failed: {0}")]
    Failed(String),
    /// An error occurred in the platform backend.
    #[error("Platform error: {0}")]
    PlatformError(String),
}

/// Checks if biometric authentication is available on the current device.
pub async fn is_available() -> bool {
    sys::is_available().await
}

/// Request biometric authentication with a reason.
///
/// # Errors
/// Returns a [`BiometricError`] if:
/// - Biometric authentication is not available.
/// - The user cancels the authentication.
/// - Authentication fails.
pub async fn authenticate(reason: &str) -> Result<(), BiometricError> {
    sys::authenticate(reason).await
}

/// Get the available biometric type.
///
/// Returns `None` if biometrics are not available.
pub async fn get_biometric_type() -> Option<BiometricType> {
    sys::get_biometric_type().await
}
