//! Apple platform (iOS/macOS) secure storage implementation.

use crate::SecretError;
use security_framework::passwords::{self, PasswordOptions};
use security_framework_sys::base::errSecItemNotFound;

/// Save a secret to the Apple Keychain.
///
/// # Errors
/// Returns a `SecretError::Platform` if the keychain operation fails.
#[allow(clippy::unused_async)]
pub async fn set(service: &str, account: &str, password: &str) -> Result<(), SecretError> {
    passwords::set_generic_password(service, account, password.as_bytes())
        .map_err(|e| SecretError::Platform(e.to_string()))
}

/// Retrieve a secret from the Apple Keychain.
///
/// # Errors
/// Returns `SecretError::NotFound` if the secret doesn't exist,
/// or `SecretError::Platform` if the keychain operation fails.
#[allow(clippy::unused_async)]
pub async fn get(service: &str, account: &str) -> Result<String, SecretError> {
    match passwords::generic_password(PasswordOptions::new_generic_password(service, account)) {
        Ok(password) => String::from_utf8(password)
            .map_err(|e| SecretError::Platform(format!("keychain item was not UTF-8: {e}"))),
        Err(e) if e.code() == errSecItemNotFound => Err(SecretError::NotFound),
        Err(e) => Err(SecretError::Platform(e.to_string())),
    }
}

/// Delete a secret from the Apple Keychain.
///
/// # Errors
/// Returns a `SecretError::Platform` if the keychain operation fails.
/// Deleting a non-existent secret is considered success.
#[allow(clippy::unused_async)]
pub async fn delete(service: &str, account: &str) -> Result<(), SecretError> {
    match passwords::delete_generic_password(service, account) {
        Ok(()) => Ok(()),
        Err(e) if e.code() == errSecItemNotFound => Ok(()), // Deleting non-existent is success
        Err(e) => Err(SecretError::Platform(e.to_string())),
    }
}
