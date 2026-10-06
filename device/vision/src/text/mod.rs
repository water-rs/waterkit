//! The `text` capability: recognizing printed text in an image.
//!
//! [`RecognizeText`] is served natively by `Windows.Media.Ocr` on Windows.
//! The OS engine's single quality mode serves both
//! [`RecognitionLevel`] values; a requested language it lacks selects the
//! portable realization when the application carries one.

mod sys;

use icu_locale_core::LanguageIdentifier;

use crate::{
    Quad, Request, VisionError,
    sealed::{Context, Offer, Pass, Plan, Realization, Sealed},
};

/// The languages the native text realization serves on this device.
pub fn native_languages() -> Vec<LanguageIdentifier> {
    sys::recognizer_languages()
}

/// What a text recognition request spends for its result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RecognitionLevel {
    /// Lower latency at lower recall.
    Fast,
    /// The serving realization's highest quality.
    #[default]
    Accurate,
}

/// A word recognized in an image.
#[derive(Debug, Clone, PartialEq)]
pub struct TextWord {
    /// The recognized text.
    pub text: String,
    /// The serving realization's confidence, if it reports one.
    ///
    /// `Windows.Media.Ocr` reports no confidence, so its words carry
    /// [`None`].
    pub confidence: Option<f32>,
    /// Normalized corners in reading order: top-left, top-right,
    /// bottom-right, bottom-left of the upright image.
    pub bounds: Quad,
}

/// A line of text recognized in an image.
#[derive(Debug, Clone, PartialEq)]
pub struct TextLine {
    /// The line's text in reading order.
    pub text: String,
    /// The serving realization's confidence, if it reports one.
    ///
    /// `Windows.Media.Ocr` reports no confidence, so its lines carry
    /// [`None`].
    pub confidence: Option<f32>,
    /// Normalized corners in reading order: top-left, top-right,
    /// bottom-right, bottom-left of the upright image.
    pub bounds: Quad,
    /// The line's words in reading order.
    pub words: Vec<TextWord>,
}

/// A text recognition request over one image.
///
/// Without [`languages`](Self::languages) the request follows the user's
/// profile languages. With them, the serving realization must support every
/// requested language exactly; `Windows.Media.Ocr` drives one recognizer
/// language per image, so a request naming several languages cannot be
/// served natively.
#[derive(Debug, Clone, Default)]
pub struct RecognizeText {
    languages: Vec<LanguageIdentifier>,
    level: RecognitionLevel,
}

impl RecognizeText {
    /// Creates a request in the user's profile languages.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Restricts recognition to `languages`.
    ///
    /// `Windows.Media.Ocr` recognizes exactly one language per image, so
    /// requests naming several languages are served by the portable
    /// realization when the application carries it, and fail with
    /// [`VisionError::Unsupported`] otherwise.
    #[must_use]
    pub fn languages(mut self, languages: impl IntoIterator<Item = LanguageIdentifier>) -> Self {
        self.languages = languages.into_iter().collect();
        self
    }

    /// Requests `level` of recognition effort.
    ///
    /// `Windows.Media.Ocr` has a single quality mode and serves both levels.
    #[must_use]
    pub const fn level(mut self, level: RecognitionLevel) -> Self {
        self.level = level;
        self
    }
}

impl Request for RecognizeText {
    type Output = Vec<TextLine>;
}

impl Sealed for RecognizeText {
    type Plan = TextPlan;

    fn plan(&self, context: Context<'_>) -> Result<Self::Plan, VisionError> {
        Ok(TextPlan {
            languages: self.languages.clone(),
            level: self.level,
            realization: context.select("text", &sys::offer(self), &Offer::Absent)?,
        })
    }
}

/// A text request's selected realization.
///
/// Public only because the sealed [`crate::Request`] contract names it;
/// realization code constructs it.
#[doc(hidden)]
#[derive(Debug)]
pub struct TextPlan {
    languages: Vec<LanguageIdentifier>,
    level: RecognitionLevel,
    realization: Realization,
}

impl Plan<RecognizeText> for TextPlan {
    async fn prepare(&self, _context: Context<'_>) -> Result<(), VisionError> {
        match self.realization {
            Realization::Native => sys::prepare(self),
            Realization::Portable => unreachable!("no portable text realization exists yet"),
        }
    }

    #[cfg_attr(
        target_arch = "wasm32",
        expect(
            clippy::future_not_send,
            reason = "on wasm32 wgpu devices, queues and textures are not `Send`, so neither is a future holding them"
        )
    )]
    async fn run(self, pass: &mut Pass<'_>) -> Result<Vec<TextLine>, VisionError> {
        match self.realization {
            Realization::Native => sys::recognize(pass, &self).await,
            Realization::Portable => unreachable!("no portable text realization exists yet"),
        }
    }
}
