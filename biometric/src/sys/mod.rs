#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple;
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use apple::*;

#[cfg(target_os = "android")]
pub mod android;
#[cfg(target_os = "android")]
pub use android::*;

#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows"
)))]
pub mod stub {
    use crate::{BiometricError, BiometricType};

    pub async fn is_available() -> bool {
        false
    }

    pub async fn authenticate(_reason: &str) -> Result<(), BiometricError> {
        Err(BiometricError::NotAvailable)
    }

    pub async fn get_biometric_type() -> Option<BiometricType> {
        None
    }
}
#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows"
)))]
pub use stub::*;
