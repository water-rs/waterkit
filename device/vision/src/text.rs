//! Text recognition requests and results.

use icu_locale_core::LanguageIdentifier;

use crate::{
    Quad, Request, VisionError,
    sealed::{Context, Pass, Plan, Realization, Sealed},
};

/// A request recognizing text lines in an image.
///
/// With no languages set, recognition detects languages automatically. Every
/// requested language must be served by one realization: when the device's
/// native realization lacks one, selection falls to the portable realization
/// the application carries, and without it the request fails with
/// [`VisionError::Unsupported`] naming the missing languages.
/// [`Vision::capabilities`](crate::Vision::capabilities) reports the languages
/// the native realization serves at every level.
#[derive(Debug, Clone, Default)]
pub struct RecognizeText {
    /// The languages to recognize, empty for automatic detection.
    pub(crate) languages: Vec<LanguageIdentifier>,
    /// The speed-accuracy trade-off.
    pub(crate) level: RecognitionLevel,
}

impl RecognizeText {
    /// Creates a request with automatic language detection and
    /// [`RecognitionLevel::Accurate`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Limits recognition to `languages`, given as BCP-47 identifiers.
    ///
    /// An empty list keeps automatic language detection.
    #[must_use]
    pub fn languages(mut self, languages: impl IntoIterator<Item = LanguageIdentifier>) -> Self {
        self.languages = languages.into_iter().collect();
        self
    }

    /// The recognition speed-accuracy trade-off.
    #[must_use]
    pub const fn level(mut self, level: RecognitionLevel) -> Self {
        self.level = level;
        self
    }
}

/// How a realization trades recognition speed against accuracy.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecognitionLevel {
    /// Faster recognition that skips language modeling; fewer languages may
    /// be served at this level.
    Fast,
    /// Highest accuracy, applying language models and corrections.
    #[default]
    Accurate,
}

/// One recognized text line.
#[derive(Debug)]
#[non_exhaustive]
pub struct TextLine {
    /// The recognized text.
    pub text: String,
    /// The realization's confidence in the recognized text, 0 to 1.
    pub confidence: f32,
    /// The line's corners in reading order, normalized to the upright image.
    pub bounds: Quad,
}

/// The selected realization for a [`RecognizeText`] request.
#[derive(Debug)]
pub struct TextPlan {
    languages: Vec<LanguageIdentifier>,
    level: RecognitionLevel,
    realization: Realization,
}

impl Request for RecognizeText {
    type Output = Vec<TextLine>;
}

impl Sealed for RecognizeText {
    type Plan = TextPlan;

    fn plan(&self, context: Context<'_>) -> Result<Self::Plan, VisionError> {
        let realization = context.select(
            "text recognition",
            &crate::sys::native::text_offer(&self.languages, self.level),
            &crate::sys::portable_text_offer(),
        )?;
        Ok(TextPlan {
            languages: self.languages.clone(),
            level: self.level,
            realization,
        })
    }
}

impl Plan<RecognizeText> for TextPlan {
    async fn prepare(&self, _context: Context<'_>) -> Result<(), VisionError> {
        Ok(())
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
            Realization::Native => {
                crate::sys::native::recognize_text(pass, &self.languages, self.level).await
            }
            Realization::Portable => unreachable!(
                "the portable text realization does not exist yet, so its offer is always absent and selection cannot serve it"
            ),
        }
    }
}
