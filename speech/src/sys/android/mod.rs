use crate::{RecognitionConfig, RecognitionResult, SpeechError, TtsConfig, Voice};

pub fn recognition_is_available() -> bool {
    false
}

#[derive(Debug)]
pub struct TtsInner;

impl TtsInner {
    #[allow(clippy::unused_async)]
    pub async fn new() -> Result<Self, SpeechError> {
        Err(SpeechError::PlatformError(
            "Android: use JNI context directly".into(),
        ))
    }

    pub fn available_voices(&self) -> Result<Vec<Voice>, SpeechError> {
        Err(SpeechError::NotSupported)
    }

    #[allow(clippy::unused_async)]
    pub async fn speak(&self, _text: &str, _config: &TtsConfig) -> Result<(), SpeechError> {
        Err(SpeechError::NotSupported)
    }

    pub fn stop(&self) {}

    pub fn is_speaking(&self) -> bool {
        false
    }
}

#[derive(Debug)]
pub struct SpeechRecognizerInner;

impl SpeechRecognizerInner {
    #[allow(clippy::unused_async)]
    pub async fn start(
        _config: RecognitionConfig,
    ) -> Result<(Self, async_channel::Receiver<RecognitionResult>), SpeechError> {
        Err(SpeechError::PlatformError(
            "Android: use JNI context directly".into(),
        ))
    }

    pub fn stop(&self) {}
}
