use futures::StreamExt;
use futures::channel::mpsc;
#[cfg(target_os = "android")]
use tracing::warn;

use crate::{AppToken, OtpCapabilities, OtpError, Sender};

#[cfg_attr(
    not(target_os = "android"),
    expect(
        dead_code,
        reason = "Android callback dispatch constructs these events"
    )
)]
#[derive(Debug)]
pub enum Event {
    Started,
    Message(String),
    Timeout,
    Denied,
    Failed(String),
}

#[derive(Debug)]
pub struct Request {
    #[cfg_attr(
        not(target_os = "android"),
        expect(
            dead_code,
            reason = "Android request drops cancel their Kotlin listener"
        )
    )]
    id: u64,
    events: mpsc::UnboundedReceiver<Event>,
}

impl Request {
    #[cfg_attr(
        not(target_os = "android"),
        expect(
            dead_code,
            reason = "the Android backend creates callback request handles"
        )
    )]
    pub const fn new(id: u64, events: mpsc::UnboundedReceiver<Event>) -> Self {
        Self { id, events }
    }

    #[cfg_attr(
        not(target_os = "android"),
        expect(dead_code, reason = "the Android backend waits for listener setup")
    )]
    pub async fn next_event(&mut self) -> Option<Event> {
        self.events.next().await
    }

    pub(crate) async fn message(mut self) -> Result<String, OtpError> {
        while let Some(event) = self.events.next().await {
            match event {
                Event::Message(message) => return Ok(message),
                Event::Timeout => return Err(OtpError::Timeout),
                Event::Denied => return Err(OtpError::ConsentDenied),
                Event::Failed(message) => return Err(OtpError::Platform(message)),
                Event::Started => {}
            }
        }
        Err(OtpError::Platform(
            "OTP platform request channel closed".into(),
        ))
    }
}

#[cfg(target_os = "android")]
impl Drop for Request {
    fn drop(&mut self) {
        if let Err(error) = cancel(self.id) {
            warn!(request_id = self.id, %error, "failed to cancel OTP request");
        }
    }
}

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
use android::{
    cancel as platform_cancel, capabilities as platform_capabilities,
    start_addressed as platform_start_addressed, start_consent as platform_start_consent,
};

#[cfg(not(target_os = "android"))]
mod unsupported;
#[cfg(not(target_os = "android"))]
use unsupported::{
    capabilities as platform_capabilities, start_addressed as platform_start_addressed,
    start_consent as platform_start_consent,
};

pub async fn start_addressed() -> Result<(AppToken, Request), OtpError> {
    platform_start_addressed().await
}

pub async fn start_consent(sender: Option<Sender>) -> Result<Request, OtpError> {
    platform_start_consent(sender).await
}

#[cfg(target_os = "android")]
pub fn capabilities() -> Result<OtpCapabilities, OtpError> {
    platform_capabilities()
}

#[cfg(not(target_os = "android"))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "capability queries share the fallible public API on every platform"
)]
pub const fn capabilities() -> Result<OtpCapabilities, OtpError> {
    Ok(platform_capabilities())
}

#[cfg(target_os = "android")]
pub fn cancel(id: u64) -> Result<(), OtpError> {
    platform_cancel(id)
}
