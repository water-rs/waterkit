//! Cross-platform text-to-speech and speech recognition.
//!
//! This crate provides unified APIs for:
//! - **TTS**: Speaking text aloud with configurable voice, rate, and pitch
//! - **Speech Recognition**: Converting spoken words to text in real-time

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

mod sys;

use futures_core::Stream;
use waterkit_core::Capabilities;

/// Android-specific JNI helpers that require an `Env` and `Context`.
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
    #[allow(clippy::missing_const_for_fn)]
    pub fn stop(&self) {
        self.inner.stop();
    }

    /// Check if currently speaking.
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
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

/// Capability probe for the speech recognition subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct RecognizerCapabilities {
    /// Whether speech recognition is available on this device.
    pub available: bool,
}

impl Capabilities for RecognizerCapabilities {
    fn available(&self) -> bool {
        self.available
    }
}

/// Speech recognition engine.
#[derive(Debug)]
pub struct SpeechRecognizer {
    inner: sys::SpeechRecognizerInner,
    events: async_channel::Receiver<RecognitionResult>,
}

impl SpeechRecognizer {
    /// Probes whether speech recognition is available.
    #[must_use]
    pub fn capabilities() -> RecognizerCapabilities {
        RecognizerCapabilities {
            available: sys::recognition_is_available(),
        }
    }

    /// Starts a speech recognition session.
    ///
    /// # Errors
    ///
    /// Returns [`SpeechError`] if recognition cannot be started.
    pub async fn new(config: RecognitionConfig) -> Result<Self, SpeechError> {
        let (inner, events) = sys::SpeechRecognizerInner::start(config).await?;
        Ok(Self { inner, events })
    }

    /// Returns the stream of recognition results.
    pub fn events(&self) -> impl Stream<Item = RecognitionResult> + Send + 'static {
        self.events.clone()
    }

    /// Stops speech recognition.
    #[allow(clippy::missing_const_for_fn)]
    pub fn stop(&self) {
        self.inner.stop();
    }
}

/// Errors in speech operations.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum SpeechError {
    /// TTS/recognition not available.
    #[error("speech not available")]
    NotAvailable,
    /// Permission not granted (microphone for recognition).
    #[error("permission denied")]
    PermissionDenied,
    /// Not supported on this platform.
    #[error("not supported")]
    Unsupported,
    /// Platform error.
    #[error("platform error: {0}")]
    Platform(String),
}
