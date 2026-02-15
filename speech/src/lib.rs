//! Cross-platform text-to-speech and speech recognition.
//!
//! This crate provides unified APIs for:
//! - **TTS**: Speaking text aloud with configurable voice, rate, and pitch
//! - **Speech Recognition**: Converting spoken words to text in real-time

#![warn(missing_docs)]

mod sys;

/// Android-specific JNI helpers that require `JNIEnv` and `Context`.
#[cfg(target_os = "android")]
pub mod android {
    pub use crate::sys::init_with_context;
}

/// A voice available for text-to-speech.
#[derive(Debug, Clone)]
pub struct Voice {
    /// Platform-specific voice identifier.
    pub id: String,
    /// Human-readable voice name.
    pub name: String,
    /// Language/locale code (e.g., "en-US").
    pub language: String,
}

/// Configuration for text-to-speech.
#[derive(Debug, Clone)]
pub struct TtsConfig {
    /// Speaking rate (0.0 = slowest, 1.0 = default, 2.0 = fastest).
    pub rate: f32,
    /// Pitch multiplier (0.5 = low, 1.0 = default, 2.0 = high).
    pub pitch: f32,
    /// Volume (0.0 = silent, 1.0 = full).
    pub volume: f32,
    /// Voice to use (None = system default).
    pub voice: Option<Voice>,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            rate: 1.0,
            pitch: 1.0,
            volume: 1.0,
            voice: None,
        }
    }
}

/// Text-to-speech engine.
#[derive(Debug)]
pub struct Tts {
    inner: sys::TtsInner,
}

impl Tts {
    /// Create a new TTS engine with default configuration.
    ///
    /// # Errors
    /// Returns error if TTS is not available.
    pub async fn new() -> Result<Self, SpeechError> {
        Ok(Self {
            inner: sys::TtsInner::new().await?,
        })
    }

    /// List available voices.
    ///
    /// # Errors
    /// Returns error if voices cannot be enumerated.
    pub fn available_voices(&self) -> Result<Vec<Voice>, SpeechError> {
        self.inner.available_voices()
    }

    /// Speak the given text.
    ///
    /// # Errors
    /// Returns error if speech fails.
    pub async fn speak(&self, text: &str, config: &TtsConfig) -> Result<(), SpeechError> {
        self.inner.speak(text, config).await
    }

    /// Stop any ongoing speech.
    pub fn stop(&self) {
        self.inner.stop();
    }

    /// Check if currently speaking.
    #[must_use]
    pub fn is_speaking(&self) -> bool {
        self.inner.is_speaking()
    }
}

/// A speech recognition result (partial or final).
#[derive(Debug, Clone)]
pub struct RecognitionResult {
    /// The recognized text.
    pub text: String,
    /// Whether this is a final result.
    pub is_final: bool,
    /// Confidence score (0.0 to 1.0), if available.
    pub confidence: Option<f32>,
}

/// Configuration for speech recognition.
#[derive(Debug, Clone)]
pub struct RecognitionConfig {
    /// Language/locale code (e.g., "en-US"). None = system default.
    pub language: Option<String>,
    /// Whether to report partial results.
    pub partial_results: bool,
}

impl Default for RecognitionConfig {
    fn default() -> Self {
        Self {
            language: None,
            partial_results: true,
        }
    }
}

/// Speech recognition engine.
#[derive(Debug)]
pub struct SpeechRecognizer {
    inner: sys::SpeechRecognizerInner,
}

impl SpeechRecognizer {
    /// Check if speech recognition is available.
    #[must_use]
    pub fn is_available() -> bool {
        sys::recognition_is_available()
    }

    /// Start a speech recognition session.
    ///
    /// # Errors
    /// Returns error if recognition cannot be started.
    pub async fn start(
        config: RecognitionConfig,
    ) -> Result<(Self, async_channel::Receiver<RecognitionResult>), SpeechError> {
        let (inner, rx) = sys::SpeechRecognizerInner::start(config).await?;
        Ok((Self { inner }, rx))
    }

    /// Stop speech recognition.
    pub fn stop(&self) {
        self.inner.stop();
    }
}

/// Errors in speech operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum SpeechError {
    /// TTS/recognition not available.
    #[error("speech not available")]
    NotAvailable,
    /// Permission not granted (microphone for recognition).
    #[error("permission denied")]
    PermissionDenied,
    /// Not supported on this platform.
    #[error("not supported")]
    NotSupported,
    /// Platform error.
    #[error("platform error: {0}")]
    PlatformError(String),
}
