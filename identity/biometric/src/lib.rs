//! Cross-platform biometric authentication (`TouchID`, `FaceID`,
//! fingerprint, iris) for iOS, macOS, Android, and Windows.

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

mod sys;

use thiserror::Error;
use waterkit_core::Capabilities;

/// Android-specific JNI helpers that require a `JNIEnv` and `Context`.
#[cfg(target_os = "android")]
pub mod android {
    pub use crate::sys::android::{authenticate_with_context, init};
}

/// Type of biometric authentication available on the current device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BiometricType {
    /// Fingerprint authentication (`TouchID`, Android fingerprint, etc.)
    Fingerprint,
    /// Facial recognition (`FaceID`, Android Face Unlock, Windows Hello Face).
    Face,
    /// Iris scanning.
    Iris,
    /// Unknown / unspecified biometric type.
    Unknown,
}

/// Errors that can occur during biometric authentication.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BiometricError {
    /// Biometric authentication is not available on this device.
    #[error("biometric authentication is not available on this device")]
    NotAvailable,
    /// User cancelled the authentication.
    #[error("user cancelled the authentication")]
    Cancelled,
    /// Authentication failed with a specific message.
    #[error("authentication failed: {0}")]
    Failed(String),
    /// Platform-level failure (`LAError*`, `BiometricManager`, …).
    #[error("platform error: {0}")]
    Platform(String),
}

/// Capability probe result for the biometric subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct BiometricCapabilities {
    /// Whether biometric authentication is available on this device.
    pub available: bool,
    /// The detected biometric kind, when known.
    pub kind: Option<BiometricType>,
}

impl Capabilities for BiometricCapabilities {
    fn available(&self) -> bool {
        self.available
    }
}

/// Probes the biometric subsystem and returns its current capabilities.
///
/// Combines the legacy `is_available` + `get_biometric_type` queries into
/// one structured result.
pub async fn capabilities() -> BiometricCapabilities {
    let kind = sys::get_biometric_type().await;
    let available = kind.is_some() || sys::is_available().await;
    BiometricCapabilities { available, kind }
}

/// Requests biometric authentication with a reason shown to the user.
///
/// # Errors
///
/// Returns [`BiometricError::NotAvailable`] when biometrics are not set up,
/// [`BiometricError::Cancelled`] when the user dismisses the prompt,
/// [`BiometricError::Failed`] when authentication fails, or
/// [`BiometricError::Platform`] for OS-level failures.
pub async fn authenticate(reason: &str) -> Result<(), BiometricError> {
    sys::authenticate(reason).await
}
