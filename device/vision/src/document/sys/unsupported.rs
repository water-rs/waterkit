//! No native document realization exists on this platform.

use icu_locale_core::LanguageIdentifier;

use crate::{
    VisionError,
    document::{Document, DocumentPlan, RecognizeDocument},
    sealed::{Offer, Pass},
};

/// This platform has no native document recognizer.
pub const fn recognizer_languages() -> Vec<LanguageIdentifier> {
    Vec::new()
}

/// This platform has no native document recognizer.
pub const fn offer(_request: &RecognizeDocument) -> Offer {
    Offer::Absent
}

/// Native document recognition is never selected here; `offer` is always
/// [`Offer::Absent`].
pub fn prepare(_plan: &DocumentPlan) -> Result<(), VisionError> {
    unreachable!("a platform without a native document recognizer never selects it")
}

/// Native document recognition is never selected here; `offer` is always
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
    _plan: &DocumentPlan,
) -> Result<Document, VisionError> {
    unreachable!("a platform without a native document recognizer never selects it")
}
