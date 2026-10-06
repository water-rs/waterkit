//! Empty implementation for platforms without an on-device translation API.

use crate::translation::LanguagePair;
use crate::translation::{AssetStatus, TranslationCapabilities, TranslationError};

pub async fn capabilities() -> Result<TranslationCapabilities, TranslationError> {
    Ok(TranslationCapabilities::default())
}

pub async fn pair_status(_pair: &LanguagePair) -> Result<Option<AssetStatus>, TranslationError> {
    Err(TranslationError::Unavailable)
}

#[derive(Debug)]
pub enum Translator {}

impl Translator {
    #[expect(
        clippy::unused_async,
        reason = "the fallback preserves the platform async translator interface"
    )]
    pub async fn create(_pair: &LanguagePair) -> Result<Self, TranslationError> {
        Err(TranslationError::Unavailable)
    }

    #[expect(
        clippy::unused_async,
        reason = "the fallback preserves the platform async translator interface"
    )]
    pub async fn translate(&self, _texts: &[&str]) -> Result<Vec<String>, TranslationError> {
        #[expect(
            clippy::uninhabited_references,
            reason = "the fallback translator is uninhabited and cannot be called"
        )]
        match *self {}
    }
}
