//! Apple platform (iOS/macOS) secure storage implementation.

use crate::SecretError;
use keyring::Entry;

/// Save a secret to the Apple Keychain.
///
/// # Errors
/// Returns a `SecretError::System` if the keychain operation fails.
#[allow(clippy::unused_async)]
pub async fn set(service: &str, account: &str, password: &str) -> Result<(), SecretError> {
    let entry = Entry::new(service, account).map_err(|e| SecretError::System(e.to_string()))?;

    entry
        .set_password(password)
        .map_err(|e| SecretError::System(e.to_string()))
}

/// Retrieve a secret from the Apple Keychain.
///
/// # Errors
/// Returns `SecretError::NotFound` if the secret doesn't exist,
/// or `SecretError::System` if the keychain operation fails.
#[allow(clippy::unused_async)]
pub async fn get(service: &str, account: &str) -> Result<String, SecretError> {
    let entry = Entry::new(service, account).map_err(|e| SecretError::System(e.to_string()))?;

    match entry.get_password() {
        Ok(pwd) => Ok(pwd),
        Err(keyring::Error::NoEntry) => Err(SecretError::NotFound),
        Err(e) => Err(SecretError::System(e.to_string())),
    }
}

/// Delete a secret from the Apple Keychain.
///
/// # Errors
/// Returns a `SecretError::System` if the keychain operation fails.
/// Deleting a non-existent secret is considered success.
#[allow(clippy::unused_async)]
pub async fn delete(service: &str, account: &str) -> Result<(), SecretError> {
    let entry = Entry::new(service, account).map_err(|e| SecretError::System(e.to_string()))?;

    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()), // Deleting non-existent is success
        Err(e) => Err(SecretError::System(e.to_string())),
    }
}
