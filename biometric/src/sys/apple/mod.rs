//! Apple platform (iOS/macOS) biometric implementation using swift-bridge.

use crate::{BiometricError, BiometricType};

#[swift_bridge::bridge]
mod ffi {
    extern "Rust" {
        type BiometricCallback;
        fn on_success(self);
        fn on_error(self, error: String);
    }

    extern "Swift" {
        #[swift_bridge(rust_name = "biometric_is_available")]
        fn biometric_is_available() -> bool;

        #[swift_bridge(rust_name = "biometric_get_type")]
        fn biometric_get_type() -> u8; // 0: None, 1: TouchID, 2: FaceID, 3: OpticID

        #[swift_bridge(rust_name = "biometric_authenticate")]
        fn biometric_authenticate(reason: &str, callback: BiometricCallback);
    }
}

/// A callback structure for biometric authentication results.
pub struct BiometricCallback {
    sender: tokio::sync::oneshot::Sender<Result<(), BiometricError>>,
}

impl BiometricCallback {
    fn on_success(self) {
        let _ = self.sender.send(Ok(()));
    }

    fn on_error(self, error: String) {
        let _ = self.sender.send(Err(BiometricError::Failed(error)));
    }
}

/// Check if biometrics are available on Apple platforms.
#[allow(clippy::unused_async)]
pub async fn is_available() -> bool {
    ffi::biometric_is_available()
}

/// Get the biometric type on Apple platforms.
#[allow(clippy::unused_async)]
pub async fn get_biometric_type() -> Option<BiometricType> {
    match ffi::biometric_get_type() {
        1 => Some(BiometricType::Fingerprint),
        2 => Some(BiometricType::Face),
        3 => Some(BiometricType::Iris),
        _ => None,
    }
}

/// Perform biometric authentication on Apple platforms.
///
/// # Errors
/// Returns `BiometricError::NotAvailable` if biometrics are not ready,
/// or `BiometricError::PlatformError` if the channel fails.
pub async fn authenticate(reason: &str) -> Result<(), BiometricError> {
    if !is_available().await {
        return Err(BiometricError::NotAvailable);
    }

    let (tx, rx) = tokio::sync::oneshot::channel();
    let callback = BiometricCallback { sender: tx };
    
    ffi::biometric_authenticate(reason, callback);

    rx.await.unwrap_or_else(|_| Err(BiometricError::PlatformError("Channel closed".to_string())))
}
