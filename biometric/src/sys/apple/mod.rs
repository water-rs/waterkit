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

pub async fn is_available() -> bool {
    ffi::biometric_is_available()
}

pub async fn get_biometric_type() -> Option<BiometricType> {
    match ffi::biometric_get_type() {
        1 => Some(BiometricType::Fingerprint),
        2 => Some(BiometricType::Face),
        3 => Some(BiometricType::Iris),
        _ => None,
    }
}

pub async fn authenticate(reason: &str) -> Result<(), BiometricError> {
    if !is_available().await {
        return Err(BiometricError::NotAvailable);
    }

    let (tx, rx) = tokio::sync::oneshot::channel();
    let callback = BiometricCallback { sender: tx };
    
    ffi::biometric_authenticate(reason, callback);

    rx.await.unwrap_or(Err(BiometricError::PlatformError("Channel closed".to_string())))
}
