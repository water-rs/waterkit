//! One-time-code retrieval from addressed SMS and user consent.
//!
//! Addressed mode asks the server to include the string returned by
//! [`AddressedRequest::token`] in one SMS. Android uses the SMS Retriever API
//! when Google Play services is available and otherwise uses the framework
//! app-specific SMS token API when the device has telephony messaging.
//! The SMS Retriever message must contain the returned token and is limited
//! to 140 bytes. The app-specific-token message must also contain its returned
//! token; the Retriever-specific 140-byte limit does not apply to it. Neither
//! realization needs the `READ_SMS` permission.
//!
//! SMS Retriever and SMS User Consent time out after Play services' five-minute
//! window. App-specific-token requests have no system deadline and remain
//! pending until a message arrives or the request is dropped; callers should
//! apply their own timeout when needed.
//!
//! Consent mode asks the user to grant access to one incoming SMS, optionally
//! filtered by [`Sender`]. Android's consent flow requires the
//! [`ndk_context`] context to be an `androidx.activity.ComponentActivity`.
//! `WaterUI`'s `HydrolysisActivity` and this repository's Android harness
//! `AppCompatActivity` satisfy that requirement.
//!
//! Apple platforms cannot give an application access to incoming SMS, so this
//! crate reports addressed and consent retrieval as unavailable there. iOS
//! and macOS instead offer a code above the keyboard for a one-time-code text
//! field; that field belongs to `WaterUI`
//! ([water-rs/waterui#1874](https://github.com/water-rs/waterui/issues/1874)).
//! Windows, Linux, and wasm are unavailable as well.
//!
//! A request deliberately uses `start()` followed by `message()` rather than
//! the repository's usual `Type::new` plus `events()` convention. Every
//! addressed or consent request produces exactly one SMS, not a stream, and
//! separating setup from retrieval lets addressed callers obtain the server
//! token before sending that one message.

#![warn(missing_docs)]

mod sys;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use sha2::{Digest, Sha256};
use std::fmt;

/// The OTP facilities exposed by the current platform and device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct OtpCapabilities {
    /// Whether at least one addressed or consent realization is available.
    pub available: bool,
    /// The addressed realization selected by this device, if any.
    pub addressed: Option<AddressedRealization>,
    /// Whether Android SMS User Consent is available.
    pub consent: bool,
    /// Whether Apple offers one-time-code autofill above the keyboard.
    ///
    /// This is true only on iOS and macOS. The crate cannot read the incoming
    /// SMS on those platforms.
    pub one_time_code_autofill: bool,
}

impl waterkit_core::Capabilities for OtpCapabilities {
    fn available(&self) -> bool {
        self.available
    }
}

/// The addressed SMS realization selected for the current device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AddressedRealization {
    /// Google Play services' SMS Retriever API.
    SmsRetriever,
    /// Android's framework app-specific SMS token API.
    AppSpecificToken,
}

/// Returns the OTP capabilities of the current platform and device.
///
/// On Android, the application's `ndk_context` must already be initialized.
///
/// # Errors
///
/// Returns [`OtpError::Platform`] if the Android capability query fails.
#[cfg(target_os = "android")]
pub fn capabilities() -> Result<OtpCapabilities, OtpError> {
    sys::capabilities()
}

/// Returns the OTP capabilities of the current platform and device.
///
/// On Android, the application's `ndk_context` must already be initialized.
///
/// # Errors
///
/// Returns [`OtpError::Platform`] if the Android capability query fails.
#[cfg(not(target_os = "android"))]
pub const fn capabilities() -> Result<OtpCapabilities, OtpError> {
    sys::capabilities()
}

/// The server token to embed in an addressed SMS.
///
/// A token is exactly 11 ASCII characters from the standard or URL-safe
/// base64 alphabet. The value differs between Android realizations, so callers
/// must use the value returned by [`AddressedRequest::start`] for every
/// request rather than hard-coding a hash.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AppToken(String);

impl AppToken {
    /// Borrows the token string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[cfg_attr(
        all(not(target_os = "android"), not(test)),
        expect(dead_code, reason = "the Android backend validates framework tokens")
    )]
    pub(crate) fn new(value: String) -> Result<Self, OtpError> {
        if value.len() == 11
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'-' | b'_')
            })
        {
            Ok(Self(value))
        } else {
            Err(OtpError::Platform(
                "Android returned an invalid 11-character SMS token".into(),
            ))
        }
    }
}

impl fmt::Display for AppToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// An optional sender filter for SMS User Consent.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Sender(String);

impl Sender {
    /// Creates a sender filter.
    ///
    /// # Errors
    ///
    /// Returns [`OtpError::InvalidSender`] when the value is empty or contains
    /// any Unicode whitespace.
    pub fn new(value: impl Into<String>) -> Result<Self, OtpError> {
        let value = value.into();
        if value.is_empty() || value.chars().any(char::is_whitespace) {
            return Err(OtpError::InvalidSender(value));
        }
        Ok(Self(value))
    }

    /// Borrows the sender filter.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An addressed request that has installed an SMS listener.
#[derive(Debug)]
pub struct AddressedRequest {
    token: AppToken,
    request: sys::Request,
}

impl AddressedRequest {
    /// Starts an addressed request and waits until the device is listening.
    ///
    /// The returned token must be embedded in the one SMS sent by the server.
    ///
    /// # Errors
    ///
    /// Returns [`OtpError::Unavailable`] when no addressed realization exists,
    /// or a platform error if querying capabilities or setting up the listener
    /// fails.
    pub async fn start() -> Result<Self, OtpError> {
        let (token, request) = sys::start_addressed().await?;
        Ok(Self { token, request })
    }

    /// Returns the token the server must put in the SMS.
    #[must_use]
    pub const fn token(&self) -> &AppToken {
        &self.token
    }

    /// Waits for the one SMS and returns its full text.
    ///
    /// Dropping this future cancels the platform listener.
    ///
    /// # Errors
    ///
    /// Returns [`OtpError::Timeout`] for an SMS Retriever timeout, or a
    /// platform error if delivery fails.
    ///
    /// SMS Retriever returns [`OtpError::Timeout`] after Play services'
    /// five-minute window. App-specific-token requests have no system deadline
    /// and remain pending until the message arrives or this request is
    /// dropped; callers should apply their own timeout when needed.
    pub async fn message(self) -> Result<String, OtpError> {
        self.request.message().await
    }
}

/// A one-message SMS User Consent request.
#[derive(Debug)]
pub struct ConsentRequest {
    request: sys::Request,
}

impl ConsentRequest {
    /// Starts SMS User Consent, optionally filtering by sender.
    ///
    /// # Errors
    ///
    /// Returns [`OtpError::Unavailable`] when User Consent is unavailable, or
    /// a platform error if querying capabilities or setting up the listener
    /// fails.
    pub async fn start(sender: Option<Sender>) -> Result<Self, OtpError> {
        Ok(Self {
            request: sys::start_consent(sender).await?,
        })
    }

    /// Waits for the one consented SMS and returns its full text.
    ///
    /// Dropping this future cancels the receiver and activity-result launcher.
    ///
    /// # Errors
    ///
    /// Returns [`OtpError::ConsentDenied`] when the user declines, or
    /// [`OtpError::Timeout`] when Play services' five-minute window expires.
    pub async fn message(self) -> Result<String, OtpError> {
        self.request.message().await
    }
}

/// Errors returned by one-time-code retrieval.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum OtpError {
    /// No requested OTP realization is available.
    #[error("one-time-code retrieval is unavailable")]
    Unavailable,
    /// Play services did not deliver a message within its five-minute window.
    #[error("one-time-code retrieval timed out after five minutes")]
    Timeout,
    /// The user declined SMS User Consent.
    #[error("SMS User Consent was denied")]
    ConsentDenied,
    /// The sender filter is empty or contains whitespace.
    #[error("invalid SMS sender: {0}")]
    InvalidSender(String),
    /// The platform failed to set up or complete a request.
    #[error("one-time-code platform error: {0}")]
    Platform(String),
}

#[cfg_attr(
    all(not(target_os = "android"), not(test)),
    expect(dead_code, reason = "the Android backend derives SMS Retriever tokens")
)]
pub(crate) fn derive_app_token(
    package_name: &str,
    certificate_der: &[u8],
) -> Result<AppToken, OtpError> {
    let input = format!("{package_name} {}", hex::encode(certificate_der));
    let digest = Sha256::digest(input.as_bytes());
    let encoded = STANDARD_NO_PAD.encode(&digest[..9]);
    AppToken::new(encoded[..11].to_owned())
}

#[cfg(test)]
mod tests {
    use super::{AddressedRealization, AppToken, OtpCapabilities, Sender, derive_app_token};

    #[test]
    fn validates_app_tokens() {
        assert!(AppToken::new("Abc123+/_-Z".to_owned()).is_ok());
        assert!(AppToken::new("too-short".to_owned()).is_err());
        assert!(AppToken::new("contains space".to_owned()).is_err());
    }

    #[test]
    fn derives_the_google_sms_retriever_token() {
        // Independent vector: python3 -c 'import base64,hashlib;print(base64.b64encode(hashlib.sha256(b"com.example.app 3003020101").digest()[:9]).decode()[:11])'
        assert_eq!(
            derive_app_token("com.example.app", &[0x30, 0x03, 0x02, 0x01, 0x01])
                .expect("fixed vector is valid")
                .as_str(),
            "WGK2jvr9ocg"
        );
    }

    #[test]
    fn rejects_invalid_senders() {
        assert_eq!(
            Sender::new("")
                .expect_err("empty sender must fail")
                .to_string(),
            "invalid SMS sender: "
        );
        assert!(Sender::new("555 1234").is_err());
        assert_eq!(
            Sender::new("5551234")
                .expect("numeric sender is valid")
                .as_str(),
            "5551234"
        );
    }

    #[test]
    fn capabilities_trait_uses_available_field() {
        let capabilities = OtpCapabilities {
            available: true,
            addressed: Some(AddressedRealization::SmsRetriever),
            consent: false,
            one_time_code_autofill: false,
        };
        assert!(waterkit_core::Capabilities::available(&capabilities));
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn unsupported_capabilities_are_returned_successfully() {
        let capabilities = super::capabilities().expect("unsupported probe is infallible");
        assert!(!capabilities.available);
        assert_eq!(capabilities.addressed, None);
        assert!(!capabilities.consent);
        assert_eq!(
            capabilities.one_time_code_autofill,
            cfg!(any(target_os = "ios", target_os = "macos"))
        );
    }
}
