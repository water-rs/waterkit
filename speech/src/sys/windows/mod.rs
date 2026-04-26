use crate::{RecognitionConfig, RecognitionResult, SpeechError, TtsConfig, Voice};
use windows::Foundation::TypedEventHandler;
use windows::Globalization::Language;
use windows::Media::SpeechRecognition::{
    SpeechContinuousRecognitionCompletedEventArgs, SpeechContinuousRecognitionMode,
    SpeechContinuousRecognitionResultGeneratedEventArgs, SpeechContinuousRecognitionSession,
    SpeechRecognitionConfidence, SpeechRecognitionResultStatus,
    SpeechRecognizer as WinSpeechRecognizer,
};
use windows::Media::SpeechSynthesis::SpeechSynthesizer;

pub fn recognition_is_available() -> bool {
    WinSpeechRecognizer::SystemSpeechLanguage().is_ok()
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
        let _ = self;
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

    pub const fn stop(&self) {
        let _ = self;
    }

    pub const fn is_speaking(&self) -> bool {
        let _ = self;
        false
    }
}

#[derive(Debug)]
pub struct SpeechRecognizerInner {
    recognizer: WinSpeechRecognizer,
    session: SpeechContinuousRecognitionSession,
    result_cookie: i64,
    completed_cookie: i64,
}

const fn recognition_mode(partial_results: bool) -> SpeechContinuousRecognitionMode {
    if partial_results {
        SpeechContinuousRecognitionMode::Default
    } else {
        SpeechContinuousRecognitionMode::PauseOnRecognition
    }
}

impl SpeechRecognizerInner {
    pub async fn start(
        config: RecognitionConfig,
    ) -> Result<(Self, async_channel::Receiver<RecognitionResult>), SpeechError> {
        let recognizer = if let Some(language_tag) = config.language.as_deref() {
            let tag: windows::core::HSTRING = language_tag.into();
            let language = Language::CreateLanguage(&tag)
                .map_err(|e| SpeechError::PlatformError(e.to_string()))?;
            WinSpeechRecognizer::Create(&language).map_err(|e| {
                SpeechError::PlatformError(format!(
                    "create recognizer for language {language_tag}: {e}"
                ))
            })?
        } else {
            WinSpeechRecognizer::new().map_err(|e| SpeechError::PlatformError(e.to_string()))?
        };

        let compilation = recognizer
            .CompileConstraintsAsync()
            .map_err(|e| SpeechError::PlatformError(format!("CompileConstraintsAsync: {e}")))?
            .await
            .map_err(|e| SpeechError::PlatformError(format!("compile constraints await: {e}")))?;
        let compilation_status = compilation
            .Status()
            .map_err(|e| SpeechError::PlatformError(format!("compile status: {e}")))?;
        if compilation_status != SpeechRecognitionResultStatus::Success {
            return Err(SpeechError::NotAvailable);
        }

        let session = recognizer.ContinuousRecognitionSession().map_err(|e| {
            SpeechError::PlatformError(format!("ContinuousRecognitionSession: {e}"))
        })?;

        let (tx, rx) = async_channel::bounded(32);
        let tx_for_results = tx.clone();
        let mode = recognition_mode(config.partial_results);
        let result_cookie = session
            .ResultGenerated(&TypedEventHandler::new(
                move |_sender: windows::core::Ref<SpeechContinuousRecognitionSession>,
                      args: windows::core::Ref<
                    SpeechContinuousRecognitionResultGeneratedEventArgs,
                >|
                      -> windows::core::Result<()> {
                    let Some(args) = args.as_ref() else {
                        return Ok(());
                    };
                    let result = args.Result()?;
                    let status = result.Status()?;
                    if status != SpeechRecognitionResultStatus::Success {
                        return Ok(());
                    }

                    let text = result.Text()?.to_string();
                    if text.is_empty() {
                        return Ok(());
                    }

                    let confidence = match result.Confidence()? {
                        SpeechRecognitionConfidence::High => Some(0.9),
                        SpeechRecognitionConfidence::Medium => Some(0.6),
                        SpeechRecognitionConfidence::Low => Some(0.3),
                        SpeechRecognitionConfidence::Rejected => Some(0.0),
                        _ => None,
                    };
                    let _ = tx_for_results.try_send(RecognitionResult {
                        text,
                        is_final: true,
                        confidence,
                    });
                    Ok(())
                },
            ))
            .map_err(|e| SpeechError::PlatformError(format!("register ResultGenerated: {e}")))?;

        let tx_for_completed = tx.clone();
        let completed_cookie = session
            .Completed(&TypedEventHandler::new(
                move |_sender: windows::core::Ref<SpeechContinuousRecognitionSession>,
                      _args: windows::core::Ref<SpeechContinuousRecognitionCompletedEventArgs>|
                      -> windows::core::Result<()> {
                    let _ = tx_for_completed.close();
                    Ok(())
                },
            ))
            .map_err(|e| SpeechError::PlatformError(format!("register Completed: {e}")))?;

        if let Err(e) = session
            .StartWithModeAsync(mode)
            .map_err(|error| SpeechError::PlatformError(format!("StartWithModeAsync: {error}")))?
            .await
            .map_err(|error| {
                SpeechError::PlatformError(format!("StartWithModeAsync await: {error}"))
            })
        {
            let _ = session.RemoveResultGenerated(result_cookie);
            let _ = session.RemoveCompleted(completed_cookie);
            return Err(e);
        }

        Ok((
            Self {
                recognizer,
                session,
                result_cookie,
                completed_cookie,
            },
            rx,
        ))
    }

    pub fn stop(&self) {
        let _ = self.session.RemoveResultGenerated(self.result_cookie);
        let _ = self.session.RemoveCompleted(self.completed_cookie);
        let _ = self.session.CancelAsync();
        let _ = self.recognizer.StopRecognitionAsync();
    }
}
