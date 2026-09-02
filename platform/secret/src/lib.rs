//! Cross-platform secure storage.
//!
//! Provides a unified API for storing secrets securely across iOS, macOS,
//! Android, Windows, and Linux.

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

mod sys;

/// Android-specific JNI helpers that require an explicit `Env` and `Context`.
#[cfg(target_os = "android")]
pub mod android {
    pub use crate::sys::android::{delete_with_context, get_with_context, set_with_context};
}

/// Errors that can occur when accessing secrets.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SecretError {
    /// The secret could not be found.
    #[error("secret not found")]
    NotFound,
    /// Permission was denied.
    #[error("permission denied")]
    PermissionDenied,
    /// Platform-level failure (keychain, credential manager, libsecret, …).
    #[error("platform error: {0}")]
    Platform(String),
    /// Invalid input (e.g. empty service/account).
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

/// A manager for secure secret storage.
#[derive(Debug)]
pub struct SecretManager;

fn validate_identifiers(service: &str, account: &str) -> Result<(), SecretError> {
    if service.is_empty() {
        return Err(SecretError::InvalidInput("service cannot be empty".into()));
    }
    if account.is_empty() {
        return Err(SecretError::InvalidInput("account cannot be empty".into()));
    }
    Ok(())
}

impl SecretManager {
    /// Save a secret.
    ///
    /// # Errors
    /// Returns a `SecretError` if:
    /// - The service name is empty.
    /// - The account name is empty.
    /// - The underlying system storage fails.
    pub async fn set(service: &str, account: &str, password: &str) -> Result<(), SecretError> {
        validate_identifiers(service, account)?;
        sys::set(service, account, password).await
    }

    /// Retrieve a secret.
    ///
    /// # Errors
    /// Returns a `SecretError` if:
    /// - The service name is empty.
    /// - The account name is empty.
    /// - The secret is not found.
    /// - The underlying system storage fails.
    pub async fn get(service: &str, account: &str) -> Result<String, SecretError> {
        validate_identifiers(service, account)?;
        sys::get(service, account).await
    }

    /// Delete a secret.
    ///
    /// # Errors
    /// Returns a `SecretError` if:
    /// - The service name is empty.
    /// - The account name is empty.
    /// - The underlying system storage fails.
    pub async fn delete(service: &str, account: &str) -> Result<(), SecretError> {
        validate_identifiers(service, account)?;
        sys::delete(service, account).await
    }
}
