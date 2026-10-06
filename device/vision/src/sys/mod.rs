//! Platform dispatch for native and portable realizations.
//!
//! `native` names the operating system's own implementation when the device
//! has one. Portable realizations are application-carried implementations
//! selected by the `portable-*` features (#130, #132); none exists yet, so
//! every portable offer is [`Offer::Absent`].

#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple;

#[cfg(all(
    any(target_os = "ios", target_os = "macos"),
    any(feature = "barcode", feature = "text")
))]
pub use apple as native;

#[cfg(not(any(target_os = "ios", target_os = "macos")))]
pub mod native {
    //! Stub offers for platforms without a native realization: selection
    //! falls to portable or `Unsupported`, and the run functions are never
    //! reached because `select` cannot serve `Native` from an absent offer.

    /// There is no native realization on this platform.
    #[cfg(feature = "barcode")]
    pub const fn barcodes_offer(
        _requested: enumset::EnumSet<crate::Symbology>,
    ) -> crate::sealed::Offer {
        crate::sealed::Offer::Absent
    }

    /// There is no native realization on this platform.
    #[cfg(feature = "barcode")]
    pub const fn supported_symbologies() -> enumset::EnumSet<crate::Symbology> {
        enumset::EnumSet::empty()
    }

    /// There is no native realization on this platform, so `select` can
    /// never resolve a native realization here.
    #[cfg(feature = "barcode")]
    pub async fn detect_barcodes(
        _pass: &mut crate::sealed::Pass<'_>,
        _symbologies: enumset::EnumSet<crate::Symbology>,
    ) -> Result<Vec<crate::Barcode>, crate::VisionError> {
        unreachable!("selection cannot serve a native realization this platform does not have")
    }

    /// There is no native realization on this platform.
    #[cfg(feature = "text")]
    pub const fn text_offer(
        _languages: &[icu_locale_core::LanguageIdentifier],
        _level: crate::RecognitionLevel,
    ) -> crate::sealed::Offer {
        crate::sealed::Offer::Absent
    }

    /// There is no native realization on this platform.
    #[cfg(feature = "text")]
    pub fn supported_languages(
        _level: crate::RecognitionLevel,
    ) -> &'static [icu_locale_core::LanguageIdentifier] {
        &[]
    }

    /// There is no native realization on this platform, so `select` can
    /// never resolve a native realization here.
    #[cfg(feature = "text")]
    pub async fn recognize_text(
        _pass: &mut crate::sealed::Pass<'_>,
        _languages: &[icu_locale_core::LanguageIdentifier],
        _level: crate::RecognitionLevel,
    ) -> Result<Vec<crate::TextLine>, crate::VisionError> {
        unreachable!("selection cannot serve a native realization this platform does not have")
    }
}

/// Barcodes the build serves, used to answer [`Vision::capabilities`].
#[cfg(feature = "barcode")]
pub fn barcodes_capability() -> crate::RealizationSet<enumset::EnumSet<crate::Symbology>> {
    crate::RealizationSet {
        native: native::supported_symbologies(),
        portable: crate::Portable::Absent,
    }
}

/// Languages served at every [`RecognitionLevel`], used to answer
/// [`Vision::capabilities`].
///
/// A capability report advertises only what every request can rely on:
/// languages a level does not serve are still selectable through
/// [`native::text_offer`], but they are not in the advertised set.
#[cfg(feature = "text")]
pub fn text_capability() -> crate::RealizationSet<Vec<icu_locale_core::LanguageIdentifier>> {
    let fast = native::supported_languages(crate::RecognitionLevel::Fast);
    let accurate = native::supported_languages(crate::RecognitionLevel::Accurate);
    crate::RealizationSet {
        native: accurate
            .iter()
            .filter(|language| fast.contains(language))
            .cloned()
            .collect(),
        portable: crate::Portable::Absent,
    }
}

/// The portable barcode realization is application-carried code that does
/// not exist yet; its offer is always absent.
#[cfg(feature = "barcode")]
pub const fn portable_barcodes_offer() -> crate::sealed::Offer {
    crate::sealed::Offer::Absent
}

/// The portable text realization is application-carried code that does not
/// exist yet; its offer is always absent.
#[cfg(feature = "text")]
pub const fn portable_text_offer() -> crate::sealed::Offer {
    crate::sealed::Offer::Absent
}
