//! Types and operations for on-device text translation.

use std::future::Future;

use crate::{LanguageIdentifier, sys};
use waterkit_core::Capabilities;

/// A directed pair of languages.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LanguagePair {
    source: LanguageIdentifier,
    target: LanguageIdentifier,
}

impl LanguagePair {
    /// Creates a translation pair from its source and target languages.
    #[must_use]
    pub const fn new(source: LanguageIdentifier, target: LanguageIdentifier) -> Self {
        Self { source, target }
    }

    /// The language to translate from.
    #[must_use]
    pub const fn source(&self) -> &LanguageIdentifier {
        &self.source
    }

    /// The language to translate to.
    #[must_use]
    pub const fn target(&self) -> &LanguageIdentifier {
        &self.target
    }
}

impl std::fmt::Display for LanguagePair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} to {}", self.source, self.target)
    }
}

/// The installation state of a language pair's translation assets.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetStatus {
    /// The translation assets are installed and ready to use.
    Installed,
    /// The translation assets can be downloaded by the operating system.
    NeedsDownload,
    /// The translation assets are currently downloading. This status only
    /// occurs on Android.
    Downloading,
}

/// The status of one supported translation pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairCapability {
    pair: LanguagePair,
    status: AssetStatus,
}

impl PairCapability {
    /// The supported source and target languages.
    #[must_use]
    pub const fn pair(&self) -> &LanguagePair {
        &self.pair
    }

    /// The installation status of the translation assets.
    #[must_use]
    pub const fn status(&self) -> AssetStatus {
        self.status
    }

    #[cfg(any(target_os = "android", target_os = "ios", target_os = "macos", test))]
    pub(crate) const fn new(pair: LanguagePair, status: AssetStatus) -> Self {
        Self { pair, status }
    }
}

/// Translation pairs supported by the device's on-device service.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranslationCapabilities {
    pairs: Vec<PairCapability>,
}

impl TranslationCapabilities {
    /// The supported directed translation pairs.
    #[must_use]
    pub fn pairs(&self) -> &[PairCapability] {
        &self.pairs
    }

    /// Returns the asset status for a supported pair.
    #[must_use]
    pub fn status(&self, pair: &LanguagePair) -> Option<AssetStatus> {
        self.pairs
            .iter()
            .find(|capability| capability.pair == *pair)
            .map(PairCapability::status)
    }

    #[cfg(any(target_os = "android", target_os = "ios", target_os = "macos", test))]
    pub(crate) const fn new(pairs: Vec<PairCapability>) -> Self {
        Self { pairs }
    }
}

impl Capabilities for TranslationCapabilities {
    fn available(&self) -> bool {
        !self.pairs.is_empty()
    }
}

/// An error produced by on-device translation.
#[non_exhaustive]
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TranslationError {
    /// On-device translation is not available on this device.
    #[error("on-device translation is not available on this device")]
    Unavailable,
    /// The device does not support the requested source and target languages.
    #[error("the device does not support translation from {0}")]
    UnsupportedPair(LanguagePair),
    /// The translation assets are not installed and need downloading.
    #[error("the language assets for translation from {0} are not installed and need downloading")]
    NeedsDownload(LanguagePair),
    /// The translation assets are still downloading.
    #[error("the language assets for translation from {0} are still downloading")]
    Downloading(LanguagePair),
    /// The operating system's translation service failed.
    #[error("translation failed: {0}")]
    Platform(String),
}

/// Queries the language pairs supported by the system translation service.
///
/// Pairs the service does not support are omitted. Platforms and operating
/// system versions without the translation framework return an empty result.
///
/// # Errors
///
/// Returns [`TranslationError::Platform`] if the native service returns
/// malformed data or fails unexpectedly.
pub async fn capabilities() -> Result<TranslationCapabilities, TranslationError> {
    sys::capabilities().await
}

/// A reusable translator for one installed language pair.
#[derive(Debug)]
pub struct Translator {
    pair: LanguagePair,
    inner: sys::Translator,
}

impl Translator {
    /// Creates a translator when the system has installed assets for this pair.
    ///
    /// # Errors
    ///
    /// Returns [`TranslationError::UnsupportedPair`] if the device does not
    /// support the pair, [`TranslationError::NeedsDownload`] or
    /// [`TranslationError::Downloading`] if its assets are not ready, and
    /// [`TranslationError::Unavailable`] if translation is unavailable.
    pub async fn new(
        source: LanguageIdentifier,
        target: LanguageIdentifier,
    ) -> Result<Self, TranslationError> {
        let pair = LanguagePair::new(source, target);
        match sys::pair_status(&pair).await? {
            None => Err(TranslationError::UnsupportedPair(pair)),
            Some(AssetStatus::NeedsDownload) => Err(TranslationError::NeedsDownload(pair)),
            Some(AssetStatus::Downloading) => Err(TranslationError::Downloading(pair)),
            Some(AssetStatus::Installed) => {
                let inner = sys::Translator::create(&pair).await?;
                Ok(Self { pair, inner })
            }
        }
    }

    /// The source and target languages for this translator.
    #[must_use]
    pub const fn pair(&self) -> &LanguagePair {
        &self.pair
    }

    /// Translates a single string.
    ///
    /// The Android system translation service sets no deadline. Dropping an
    /// in-flight translation future cancels the request, so callers should
    /// bound it with their runtime's timeout.
    ///
    /// # Errors
    ///
    /// Returns an error if translation fails.
    pub async fn translate(&self, text: &str) -> Result<String, TranslationError> {
        let mut translated = self.translate_batch(&[text]).await?;
        Ok(translated.remove(0))
    }

    /// Translates strings in input order.
    ///
    /// An empty slice returns an empty vector without calling the platform.
    /// The Android system translation service sets no deadline. Dropping an
    /// in-flight future cancels the request, so callers should bound it with
    /// their runtime's timeout.
    ///
    /// # Errors
    ///
    /// Returns an error if translation fails or the platform returns a
    /// different number of results than requested.
    pub fn translate_batch<S: AsRef<str>>(
        &self,
        texts: &[S],
    ) -> impl Future<Output = Result<Vec<String>, TranslationError>> + Send + '_ {
        let texts = texts
            .iter()
            .map(|text| text.as_ref().to_owned())
            .collect::<Vec<_>>();

        async move {
            if texts.is_empty() {
                return Ok(Vec::new());
            }

            let text_refs = texts.iter().map(String::as_str).collect::<Vec<_>>();
            self.inner.translate(&text_refs).await
        }
    }
}

#[cfg(target_os = "android")]
pub mod android {
    //! Android translation settings and capability updates.

    use futures::{Stream, task::Poll};
    use std::{pin::Pin, task::Context};

    use super::TranslationError;
    use crate::sys;

    pub use crate::sys::android::CapabilityUpdate;

    /// A stream of Android translation capability changes.
    #[derive(Debug)]
    pub struct CapabilityUpdates {
        id: i64,
        receiver: Pin<Box<async_channel::Receiver<Result<CapabilityUpdate, TranslationError>>>>,
    }

    impl CapabilityUpdates {
        pub(crate) fn new(
            id: i64,
            receiver: async_channel::Receiver<Result<CapabilityUpdate, TranslationError>>,
        ) -> Self {
            Self {
                id,
                receiver: Box::pin(receiver),
            }
        }
    }

    impl Stream for CapabilityUpdates {
        type Item = Result<CapabilityUpdate, TranslationError>;

        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let this = self.get_mut();
            this.receiver.as_mut().poll_next(cx)
        }
    }

    impl Drop for CapabilityUpdates {
        fn drop(&mut self) {
            sys::android::remove_capability_listener(self.id);
        }
    }

    /// Opens the operating system's translation asset settings.
    ///
    /// # Errors
    ///
    /// Returns [`TranslationError::Unavailable`] if Android is older than API
    /// 31, the system service or settings intent is unavailable, or sending the
    /// intent fails.
    pub fn open_download_settings() -> Result<(), TranslationError> {
        sys::android::open_download_settings()
    }

    /// Subscribes to changes in supported Android translation pairs.
    ///
    /// # Errors
    ///
    /// Returns [`TranslationError::Unavailable`] if Android is older than API
    /// 31 or has no translation service.
    pub fn capability_updates() -> Result<CapabilityUpdates, TranslationError> {
        sys::android::capability_updates()
    }
}

#[cfg(all(
    test,
    not(any(target_os = "android", target_os = "ios", target_os = "macos"))
))]
mod fallback_tests {
    use futures::executor::block_on;
    use icu_locale_core::langid;

    use super::*;

    #[test]
    fn fallback_is_unavailable() {
        let capabilities = block_on(capabilities()).expect("fallback capabilities");
        assert!(!capabilities.available());
        assert!(capabilities.pairs().is_empty());
        assert!(matches!(
            block_on(Translator::new(langid!("en"), langid!("de"))),
            Err(TranslationError::Unavailable)
        ));
    }
}

#[cfg(test)]
mod tests {
    use icu_locale_core::langid;

    use super::*;

    #[test]
    fn displays_language_pair() {
        assert_eq!(
            LanguagePair::new(langid!("en"), langid!("de")).to_string(),
            "en to de"
        );
    }

    #[test]
    fn error_messages_name_the_pair() {
        let pair = LanguagePair::new(langid!("en"), langid!("de"));
        assert_eq!(
            TranslationError::Unavailable.to_string(),
            "on-device translation is not available on this device"
        );
        assert_eq!(
            TranslationError::UnsupportedPair(pair.clone()).to_string(),
            "the device does not support translation from en to de"
        );
        assert_eq!(
            TranslationError::NeedsDownload(pair.clone()).to_string(),
            "the language assets for translation from en to de are not installed and need downloading"
        );
        assert_eq!(
            TranslationError::Downloading(pair).to_string(),
            "the language assets for translation from en to de are still downloading"
        );
        assert_eq!(
            TranslationError::Platform("native failure".into()).to_string(),
            "translation failed: native failure"
        );
    }

    #[test]
    fn capability_status_and_availability_follow_pairs() {
        let pair = LanguagePair::new(langid!("en"), langid!("de"));
        let capabilities = TranslationCapabilities::new(vec![PairCapability::new(
            pair.clone(),
            AssetStatus::Installed,
        )]);
        assert!(capabilities.available());
        assert_eq!(capabilities.status(&pair), Some(AssetStatus::Installed));
        assert_eq!(
            capabilities.status(&LanguagePair::new(langid!("en"), langid!("fr"))),
            None
        );
        assert!(!TranslationCapabilities::default().available());
    }
}
