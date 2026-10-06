//! No native text realization exists on this platform.

use std::future::Future;

use icu_locale_core::LanguageIdentifier;

use crate::{
    RecognizeText, TextLine, VisionError,
    sealed::{Offer, Pass},
    text::TextPlan,
};

/// This platform has no native text recognizer.
pub const fn recognizer_languages() -> Vec<LanguageIdentifier> {
    Vec::new()
}

/// This platform has no native text recognizer.
pub const fn offer(_request: &RecognizeText) -> Offer {
    Offer::Absent
}

/// Native text is never selected here; `offer` is always
/// [`Offer::Absent`].
pub fn prepare(_plan: &TextPlan) -> Result<(), VisionError> {
    unreachable!("the fallback text realization is never selected")
}

/// Native text is never selected here; `offer` is always
/// [`Offer::Absent`].
pub fn recognize(
    _pass: &mut Pass<'_>,
    _plan: &TextPlan,
) -> impl Future<Output = Result<Vec<TextLine>, VisionError>> {
    async { unreachable!("the fallback text realization is never selected") }
}
