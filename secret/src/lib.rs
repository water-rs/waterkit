//! Cross-platform secure storage.
//!
//! This crate provides a unified API for storing secrets securely across
//! iOS, macOS, Android, Windows, and Linux platforms.

#![warn(missing_docs)]

/// Platform-specific implementations.
mod sys;

/// Errors that can occur when accessing secrets.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// The secret could not be found.
    #[error("secret not found")]
    NotFound,
    /// Permission was denied.
    #[error("permission denied")]
    PermissionDenied,
    /// An underlying system error occurred.
    #[error("system error: {0}")]
    System(String),
    /// Invalid input (e.g. empty service/account).
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

/// A manager for secure secret storage.
#[derive(Debug)]
pub struct SecretManager;

impl SecretManager {
    /// Save a secret.
    ///
    /// # Errors
    /// Returns a `SecretError` if:
    /// - The service name is empty.
    /// - The underlying system storage fails.
    pub async fn set(service: &str, account: &str, password: &str) -> Result<(), SecretError> {
        if service.is_empty() {
            return Err(SecretError::InvalidInput("service cannot be empty".into()));
        }
        sys::set(service, account, password).await
    }

    /// Retrieve a secret.
    ///
    /// # Errors
    /// Returns a `SecretError` if:
    /// - The service name is empty.
    /// - The secret is not found.
    /// - The underlying system storage fails.
    pub async fn get(service: &str, account: &str) -> Result<String, SecretError> {
        if service.is_empty() {
            return Err(SecretError::InvalidInput("service cannot be empty".into()));
        }
        sys::get(service, account).await
    }

    /// Delete a secret.
    ///
    /// # Errors
    /// Returns a `SecretError` if:
    /// - The service name is empty.
    /// - The underlying system storage fails.
    pub async fn delete(service: &str, account: &str) -> Result<(), SecretError> {
        if service.is_empty() {
            return Err(SecretError::InvalidInput("service cannot be empty".into()));
        }
        sys::delete(service, account).await
    }
}
