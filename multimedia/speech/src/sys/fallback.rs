use crate::{RecognitionConfig, RecognitionResult, SpeechError, TtsConfig, Voice};

#[expect(
    clippy::missing_const_for_fn,
    reason = "the facade calls every platform's backend through the same non-const signature; only this unsupported-platform shim could be const"
)]
pub fn recognition_is_available() -> bool {
    false
}

#[derive(Debug)]
pub struct TtsInner;

#[expect(
    clippy::unused_self,
    clippy::missing_const_for_fn,
    reason = "this unsupported-platform shim keeps no state and computes nothing, but the facade calls every platform's backend through the same non-const `&self` methods"
)]
impl TtsInner {
    #[allow(clippy::unused_async)]
    pub async fn new() -> Result<Self, SpeechError> {
        Err(SpeechError::Unsupported)
    }

    #[expect(
        clippy::unused_async,
        reason = "the async signature is part of the crate API surface and other platforms await here"
    )]
    pub async fn available_voices(&self) -> Result<Vec<Voice>, SpeechError> {
        Err(SpeechError::Unsupported)
    }

    #[allow(clippy::unused_async)]
    pub async fn speak(&self, _text: &str, _config: &TtsConfig) -> Result<(), SpeechError> {
        Err(SpeechError::Unsupported)
    }

    pub fn stop(&self) {}

    #[expect(
        clippy::unused_async,
        reason = "the async signature is part of the crate API surface and other platforms await here"
    )]
    pub async fn is_speaking(&self) -> bool {
        false
    }
}

#[derive(Debug)]
pub struct SpeechRecognizerInner;

#[expect(
    clippy::unused_self,
    clippy::missing_const_for_fn,
    reason = "this unsupported-platform shim keeps no state and computes nothing, but the facade calls every platform's backend through the same non-const `&self` methods"
)]
impl SpeechRecognizerInner {
    #[allow(clippy::unused_async)]
    pub async fn start(
        _config: RecognitionConfig,
    ) -> Result<(Self, async_channel::Receiver<RecognitionResult>), SpeechError> {
        Err(SpeechError::Unsupported)
    }

    pub fn stop(&self) {}
}
