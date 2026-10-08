//! No native text realization exists on this platform.

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
#[expect(
    clippy::unused_async,
    reason = "keeps the signature every platform's native realization shares"
)]
pub async fn prepare(_plan: &TextPlan) -> Result<(), VisionError> {
    unreachable!("a platform without a native text recognizer never selects it")
}

/// Native text is never selected here; `offer` is always
/// [`Offer::Absent`].
#[expect(
    clippy::unused_async,
    reason = "keeps the signature every platform's native realization shares"
)]
#[cfg_attr(
    target_arch = "wasm32",
    expect(
        clippy::future_not_send,
        reason = "on wasm32 wgpu devices, queues and textures are not `Send`, so neither is a future holding them"
    )
)]
pub async fn recognize(
    _pass: &mut Pass<'_>,
    _plan: &TextPlan,
) -> Result<Vec<TextLine>, VisionError> {
    unreachable!("a platform without a native text recognizer never selects it")
}
