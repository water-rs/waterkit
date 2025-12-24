//! Platform-specific backend implementations for secure storage.

#[cfg(any(target_os = "ios", target_os = "macos"))]
/// Apple platform backend.
pub mod apple;

#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use apple::*;

#[cfg(target_os = "android")]
/// Android platform backend.
pub mod android;

#[cfg(target_os = "android")]
pub use android::*;

#[cfg(target_os = "windows")]
/// Windows platform backend.
pub mod windows;

#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(target_os = "linux")]
/// Linux platform backend.
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
/// Save a secret (fallback).
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
/// Retrieve a secret (fallback).
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
/// Delete a secret (fallback).
pub async fn delete(_service: &str, _account: &str) -> Result<(), crate::SecretError> {
    Err(crate::SecretError::System("Unsupported platform".into()))
}
