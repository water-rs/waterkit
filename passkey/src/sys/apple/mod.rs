//! Apple passkey backend via AuthenticationServices.

use async_trait::async_trait;
use tokio::sync::oneshot;

use crate::{
    AuthenticateOptions, AuthenticationResult, Availability, PasskeyError, RegisterOptions,
    RegistrationResult, authenticate_request_json, parse_authentication_response_json,
    parse_registration_response_json, register_request_json,
};

use super::PasskeyBackend;

#[swift_bridge::bridge]
mod ffi {
    extern "Rust" {
        type RegisterCallback;
        fn on_register_success(self: RegisterCallback, response_json: String);
        fn on_register_error(self: RegisterCallback, error: String);
    }

    extern "Rust" {
        type AuthenticateCallback;
        fn on_authenticate_success(self: AuthenticateCallback, response_json: String);
        fn on_authenticate_error(self: AuthenticateCallback, error: String);
    }

    extern "Swift" {
        fn passkey_is_available() -> bool;
        fn passkey_register(request_json: &str, callback: RegisterCallback);
        fn passkey_authenticate(request_json: &str, callback: AuthenticateCallback);
    }
}

pub(crate) struct PlatformBackend;

pub struct RegisterCallback {
    sender: oneshot::Sender<Result<RegistrationResult, PasskeyError>>,
}

impl RegisterCallback {
    fn on_register_success(self, response_json: String) {
        let result = parse_registration_response_json(&response_json);
        let _ = self.sender.send(result);
    }

    fn on_register_error(self, error: String) {
        let _ = self
            .sender
            .send(Err(PasskeyError::from_platform_error(error)));
    }
}

pub struct AuthenticateCallback {
    sender: oneshot::Sender<Result<AuthenticationResult, PasskeyError>>,
}

impl AuthenticateCallback {
    fn on_authenticate_success(self, response_json: String) {
        let result = parse_authentication_response_json(&response_json);
        let _ = self.sender.send(result);
    }

    fn on_authenticate_error(self, error: String) {
        let _ = self
            .sender
            .send(Err(PasskeyError::from_platform_error(error)));
    }
}

#[async_trait]
impl PasskeyBackend for PlatformBackend {
    async fn is_available(&self) -> Result<Availability, PasskeyError> {
        if ffi::passkey_is_available() {
            Ok(Availability::supported())
        } else {
            Ok(Availability::unavailable())
        }
    }

    async fn register(
        &self,
        options: &RegisterOptions,
    ) -> Result<RegistrationResult, PasskeyError> {
        let request_json = register_request_json(options)?;
        let (tx, rx) = oneshot::channel();

        ffi::passkey_register(&request_json, RegisterCallback { sender: tx });

        rx.await.unwrap_or_else(|_| {
            Err(PasskeyError::Platform(
                "apple passkey register callback channel closed".into(),
            ))
        })
    }

    async fn authenticate(
        &self,
        options: &AuthenticateOptions,
    ) -> Result<AuthenticationResult, PasskeyError> {
        let request_json = authenticate_request_json(options)?;
        let (tx, rx) = oneshot::channel();

        ffi::passkey_authenticate(&request_json, AuthenticateCallback { sender: tx });

        rx.await.unwrap_or_else(|_| {
            Err(PasskeyError::Platform(
                "apple passkey authenticate callback channel closed".into(),
            ))
        })
    }
}
