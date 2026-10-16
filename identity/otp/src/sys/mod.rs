//! OTP backend dispatch: Android's app-classpath helper realization or the
//! `NotAvailable` fallback.

#[cfg(not(target_os = "android"))]
use futures::channel::mpsc;
use futures::{Stream, StreamExt};
use std::pin::Pin;
#[cfg(target_os = "android")]
use tracing::warn;

use crate::{AppToken, OtpCapabilities, OtpError, Sender};

/// An event the platform realization reports through a request.
///
/// `Failed` carries a platform-side message; `Timeout` and `Denied` are the
/// terminal outcomes the other platforms model the same way.
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

pub struct Request {
    /// The Java `OtpRequest` the helper drives the request through. Drop
    /// cancels the registration it owns on Android.
    #[cfg(target_os = "android")]
    pub(super) handle: Option<jni::objects::Global<jni::objects::JObject<'static>>>,
    events: Pin<Box<dyn Stream<Item = Event> + Send>>,
}

impl Request {
    /// A request whose events come through `events` — on Android the
    /// `OtpRequest` handle stays in the request so dropping it cancels the
    /// platform listener.
    #[cfg_attr(
        not(target_os = "android"),
        expect(
            dead_code,
            reason = "the Android backend creates callback request handles"
        )
    )]
    #[cfg(target_os = "android")]
    pub fn new(
        handle: jni::objects::Global<jni::objects::JObject<'static>>,
        events: impl Stream<Item = Event> + Send + 'static,
    ) -> Self {
        Self {
            handle: Some(handle),
            events: Box::pin(events),
        }
    }

    /// The fallback constructor for platforms without an OTP backend.
    #[cfg(not(target_os = "android"))]
    #[expect(dead_code, reason = "the Android backend creates request handles")]
    pub fn new(events: mpsc::UnboundedReceiver<Event>) -> Self {
        Self {
            events: Box::pin(events),
        }
    }

    /// The `OtpRequest` an Android request owns its listener through.
    #[cfg(target_os = "android")]
    pub(super) const fn handle(&self) -> &jni::objects::Global<jni::objects::JObject<'static>> {
        self.handle
            .as_ref()
            .expect("an Android OTP request always carries its request object")
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
        let Some(handle) = self.handle.take() else {
            return;
        };
        if let Err(error) = cancel(handle.as_obj()) {
            warn!(%error, "failed to cancel OTP request");
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

/// Cancels the request whose `NativeChannel` is `channel`.
#[cfg(target_os = "android")]
pub fn cancel(channel: &jni::objects::JObject<'_>) -> Result<(), OtpError> {
    platform_cancel(channel)
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Request").finish_non_exhaustive()
    }
}
