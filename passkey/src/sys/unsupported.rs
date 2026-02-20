use async_trait::async_trait;

use crate::{
    AuthenticateOptions, AuthenticationResult, Availability, PasskeyError, RegisterOptions,
    RegistrationResult,
};

use super::PasskeyBackend;

pub struct PlatformBackend;

#[async_trait]
impl PasskeyBackend for PlatformBackend {
    async fn is_available(&self) -> Result<Availability, PasskeyError> {
        Ok(Availability::unavailable())
    }

    async fn register(
        &self,
        _options: &RegisterOptions,
    ) -> Result<RegistrationResult, PasskeyError> {
        Err(PasskeyError::NotSupported)
    }

    async fn authenticate(
        &self,
        _options: &AuthenticateOptions,
    ) -> Result<AuthenticationResult, PasskeyError> {
        Err(PasskeyError::NotSupported)
    }
}
