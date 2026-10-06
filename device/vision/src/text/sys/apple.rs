//! The native text realization: Apple Vision's `RecognizeTextRequest`
//! bridged through `swift-bridge` like the crate's other Apple bridges.
//!
//! The shared pass handler and bridge come from [`crate::sys::apple`]; this
//! module owns what text asks of them: the level-parameterized language
//! set, the offer, and the decode into [`TextLine`]s with per-word bounds.

use std::sync::OnceLock;

use icu_locale_core::LanguageIdentifier;

use crate::{
    RecognitionLevel, TextLine, TextWord, VisionError,
    sealed::{Offer, Pass},
    text::{RecognizeText, TextPlan},
};

use crate::sys::apple::ffi;

/// The wire value for [`RecognitionLevel::Fast`].
const FFI_LEVEL_FAST: u8 = 0;
/// The wire value for [`RecognitionLevel::Accurate`].
const FFI_LEVEL_ACCURATE: u8 = 1;

/// The wire value of `level`.
const fn ffi_level(level: RecognitionLevel) -> u8 {
    match level {
        RecognitionLevel::Fast => FFI_LEVEL_FAST,
        RecognitionLevel::Accurate => FFI_LEVEL_ACCURATE,
    }
}

/// Languages Vision serves at `level`, fetched once.
fn supported_languages(level: RecognitionLevel) -> &'static [LanguageIdentifier] {
    static LANGUAGES: OnceLock<[Vec<LanguageIdentifier>; 2]> = OnceLock::new();
    let [fast, accurate] = LANGUAGES.get_or_init(|| {
        let load = |level: u8| {
            let tags: Vec<String> =
                serde_json::from_str(&ffi::vision_supported_text_languages(level))
                    .expect("the bridge reports a JSON string array");
            tags.iter()
                .filter_map(|tag| match tag.parse::<LanguageIdentifier>() {
                    Ok(language) => Some(language),
                    Err(error) => {
                        tracing::warn!(
                            tag,
                            %error,
                            "Apple Vision returned an unparseable language tag"
                        );
                        None
                    }
                })
                .collect()
        };
        [load(FFI_LEVEL_FAST), load(FFI_LEVEL_ACCURATE)]
    });
    match level {
        RecognitionLevel::Fast => fast.as_slice(),
        RecognitionLevel::Accurate => accurate.as_slice(),
    }
}

/// `supportedRecognitionLanguages` Vision serves at every level, listed
/// exactly: the native set [`crate::Vision::capabilities`] advertises.
pub fn recognizer_languages() -> Vec<LanguageIdentifier> {
    static LANGUAGES: OnceLock<Vec<LanguageIdentifier>> = OnceLock::new();
    LANGUAGES
        .get_or_init(|| {
            let fast = supported_languages(RecognitionLevel::Fast);
            supported_languages(RecognitionLevel::Accurate)
                .iter()
                .filter(|language| fast.contains(language))
                .cloned()
                .collect()
        })
        .clone()
}

/// Whether Vision serves `request` exactly: every language it names at its
/// level, or automatic detection when it names none.
pub fn offer(request: &RecognizeText) -> Offer {
    let supported = supported_languages(request.level);
    if supported.is_empty() {
        return Offer::Absent;
    }
    let missing: Vec<String> = request
        .languages
        .iter()
        .filter(|language| !supported.contains(language))
        .map(ToString::to_string)
        .collect();
    if missing.is_empty() {
        Offer::Serves
    } else {
        Offer::Lacks(format!("recognizer languages {}", missing.join(", ")))
    }
}

/// Verifies that Vision serves every language the selected plan names.
pub fn prepare(plan: &TextPlan) -> Result<(), VisionError> {
    let supported = supported_languages(plan.level);
    let missing: Vec<String> = plan
        .languages
        .iter()
        .filter(|language| !supported.contains(language))
        .map(ToString::to_string)
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(VisionError::Unsupported(format!(
            "recognizer languages {}",
            missing.join(", ")
        )))
    }
}

/// Runs text recognition through Vision on the pass's shared handler.
pub async fn recognize(pass: &mut Pass<'_>, plan: &TextPlan) -> Result<Vec<TextLine>, VisionError> {
    let handler = pass
        .prepared::<crate::sys::apple::AppleImage>()
        .await?
        .handler;
    let tags: Vec<String> = plan.languages.iter().map(ToString::to_string).collect();
    let json = serde_json::to_string(&tags).expect("serializing strings cannot fail");
    let lines = crate::sys::apple::ffi_outcome::<WireTextLine>(|callback| {
        ffi::vision_recognize_text(handler, ffi_level(plan.level), &json, callback);
    })
    .await?;
    Ok(lines.into_iter().map(WireTextLine::into_line).collect())
}

/// A text line as the bridge reports it.
#[derive(Debug, serde::Deserialize)]
struct WireTextLine {
    /// The recognized text.
    text: String,
    /// Vision's confidence, 0 to 1.
    confidence: f32,
    /// The four upright corners in reading order, x/y interleaved.
    corners: [f32; 8],
    /// The line's words in reading order.
    words: Vec<WireTextWord>,
}

impl WireTextLine {
    fn into_line(self) -> TextLine {
        TextLine {
            text: self.text,
            confidence: Some(self.confidence),
            bounds: crate::sys::apple::wire_quad(self.corners),
            words: self
                .words
                .into_iter()
                .map(|word| TextWord {
                    text: word.text,
                    confidence: Some(word.confidence),
                    bounds: crate::sys::apple::wire_quad(word.corners),
                })
                .collect(),
        }
    }
}

/// A recognized word as the bridge reports it.
#[derive(Debug, serde::Deserialize)]
struct WireTextWord {
    /// The recognized text.
    text: String,
    /// The line candidate's confidence, 0 to 1.
    confidence: f32,
    /// The four upright corners in reading order, x/y interleaved.
    corners: [f32; 8],
}
