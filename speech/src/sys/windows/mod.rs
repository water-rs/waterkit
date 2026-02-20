use crate::{RecognitionConfig, RecognitionResult, SpeechError, TtsConfig, Voice};
use windows::Media::SpeechSynthesis::SpeechSynthesizer;

pub fn recognition_is_available() -> bool {
    false
}

#[derive(Debug)]
pub struct TtsInner {
    synth: SpeechSynthesizer,
}

impl TtsInner {
    #[allow(clippy::unused_async)]
    pub async fn new() -> Result<Self, SpeechError> {
        let synth =
            SpeechSynthesizer::new().map_err(|e| SpeechError::PlatformError(e.to_string()))?;
        Ok(Self { synth })
    }

    pub fn available_voices(&self) -> Result<Vec<Voice>, SpeechError> {
        let voices = SpeechSynthesizer::AllVoices()
            .map_err(|e| SpeechError::PlatformError(e.to_string()))?;
        let mut result = Vec::new();
        for voice in &voices {
            let id = voice.Id().map_or_else(|_| String::new(), |s| s.to_string());
            let name = voice
                .DisplayName()
                .map_or_else(|_| String::new(), |s| s.to_string());
            let language = voice
                .Language()
                .map_or_else(|_| String::new(), |s| s.to_string());
            result.push(Voice { id, name, language });
        }
        Ok(result)
    }

    pub async fn speak(&self, text: &str, _config: &TtsConfig) -> Result<(), SpeechError> {
        let text_hstring: windows::core::HSTRING = text.into();
        let stream = self
            .synth
            .SynthesizeTextToStreamAsync(&text_hstring)
            .map_err(|e| SpeechError::PlatformError(e.to_string()))?
            .await
            .map_err(|e| SpeechError::PlatformError(e.to_string()))?;
        drop(stream);
        Ok(())
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
        Err(SpeechError::NotSupported)
    }

    pub fn stop(&self) {}
}
