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

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
pub async fn set(_service: &str, _account: &str, _password: &str) -> Result<(), crate::SecretError> {
    Err(crate::SecretError::System("Unsupported platform".into()))
}

#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
pub async fn get(_service: &str, _account: &str) -> Result<String, crate::SecretError> {
    Err(crate::SecretError::System("Unsupported platform".into()))
}

#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
pub async fn delete(_service: &str, _account: &str) -> Result<(), crate::SecretError> {
    Err(crate::SecretError::System("Unsupported platform".into()))
}
