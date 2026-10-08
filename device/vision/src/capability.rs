use crate::Policy;

/// Vision capabilities available to this build on this device.
///
/// Fields arrive with the capability features (barcodes, text, scanner); each
/// reports what the device's native realization serves exactly and what the
/// application carries portably.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisionCapabilities {
    /// The barcode symbologies this build serves.
    ///
    /// On Apple `native` is `DetectBarcodesRequest.supportedSymbologies`
    /// exactly; on Android it is the `Barcode.FORMAT_*` set ML Kit's barcode
    /// engine expresses when Google Play services is usable — its module is
    /// delivered on demand at request time; it is empty on platforms without
    /// a native detector.
    #[cfg(feature = "barcode")]
    pub barcodes: RealizationSet<enumset::EnumSet<crate::Symbology>>,
    /// Text recognition: the languages each realization serves.
    ///
    /// On Apple `native` is the intersection of Vision's per-level
    /// `supportedRecognitionLanguages`; on Windows it is
    /// `OcrEngine::AvailableRecognizerLanguages` exactly; on Android it is
    /// the script identifiers ML Kit's recognizers serve (`und-Latn`,
    /// `und-Hani`, `und-Deva`, `und-Jpan`, `und-Kore`) when Google Play
    /// services is usable; it is empty on
    /// platforms without a native recognizer.
    #[cfg(feature = "text")]
    pub text: RealizationSet<Vec<icu_locale_core::LanguageIdentifier>>,
    /// Document structure recognition: the languages each realization
    /// serves.
    ///
    /// On Apple `native` is `RecognizeDocumentsRequest`'s
    /// `supportedRecognitionLanguages` exactly; it is empty on platforms
    /// without a native recognizer.
    #[cfg(feature = "document")]
    pub documents: RealizationSet<Vec<icu_locale_core::LanguageIdentifier>>,

    /// Whether this device can present the system code scanner, as
    /// [`CodeScanner::capabilities`] reports it. Present when the `scanner`
    /// feature is enabled.
    ///
    /// [`CodeScanner::capabilities`]: crate::CodeScanner::capabilities
    #[cfg(feature = "scanner")]
    pub scanner: bool,
}

impl VisionCapabilities {
    #[cfg_attr(
        all(
            any(
                not(feature = "barcode"),
                not(any(target_os = "ios", target_os = "macos", target_os = "android"))
            ),
            any(
                not(feature = "text"),
                not(any(
                    target_os = "windows",
                    target_os = "ios",
                    target_os = "macos",
                    target_os = "android"
                ))
            ),
            any(
                not(feature = "document"),
                not(any(target_os = "ios", target_os = "macos"))
            ),
            any(
                not(feature = "scanner"),
                not(any(
                    target_os = "android",
                    all(target_os = "ios", not(target_abi = "macabi"))
                ))
            )
        ),
        expect(
            clippy::missing_const_for_fn,
            reason = "a native symbology/language probe or the scanner's device-support probe is a runtime call; elsewhere the capabilities are constant"
        )
    )]
    pub(crate) fn new() -> Self {
        Self {
            #[cfg(feature = "barcode")]
            barcodes: RealizationSet {
                native: crate::barcode::native_symbologies(),
                portable: Portable::Absent,
            },
            #[cfg(feature = "text")]
            text: RealizationSet {
                native: crate::text::native_languages(),
                portable: Portable::Absent,
            },
            #[cfg(feature = "document")]
            documents: RealizationSet {
                native: crate::document::native_languages(),
                portable: Portable::Absent,
            },
            #[cfg(feature = "scanner")]
            scanner: crate::sys::scanner_available(),
        }
    }
}

/// Native and portable realizations of a capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealizationSet<T> {
    /// The native realization.
    pub native: T,
    /// The portable realization carried by the application.
    pub portable: Portable<T>,
}

#[cfg(feature = "text")]
impl<T> RealizationSet<Vec<T>> {
    /// Whether either realization serves anything.
    const fn available(&self) -> bool {
        !self.native.is_empty() || !matches!(self.portable, Portable::Absent)
    }
}

/// Availability of a portable realization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Portable<T> {
    /// The application does not carry this realization.
    Absent,
    /// The application carries the realization, but its weights are not yet
    /// on the device.
    Downloadable(T),
    /// The realization and its required weights are on the device.
    Ready(T),
}

impl waterkit_core::Capabilities for VisionCapabilities {
    /// Returns whether any capability has a realization in this build.
    fn available(&self) -> bool {
        #[cfg(feature = "barcode")]
        if !self.barcodes.native.is_empty() || !matches!(self.barcodes.portable, Portable::Absent) {
            return true;
        }
        #[cfg(feature = "text")]
        if self.text.available() {
            return true;
        }
        #[cfg(feature = "document")]
        if self.documents.available() {
            return true;
        }
        #[cfg(feature = "scanner")]
        if self.scanner {
            return true;
        }
        false
    }
}

/// Every request capability enabled by this build and whether its portable
/// realization is carried by the application.
///
/// Each second element becomes `cfg!(feature = "portable-*")` once the
/// portable realizations land (#130, #132).
///
/// The system code scanner is not a request served by [`Vision`]: it has no
/// portable realization to select, so it is absent here even when its feature
/// is enabled and [`Policy::PortableOnly`] does not constrain it.
///
/// [`Vision`]: crate::Vision
/// [`Policy::PortableOnly`]: crate::Policy::PortableOnly
pub const ENABLED: &[(&str, bool)] = &[
    #[cfg(feature = "barcode")]
    ("barcode", false),
    #[cfg(feature = "text")]
    ("text", false),
    #[cfg(feature = "document")]
    ("document", false),
];

/// Capabilities whose portable realization is not carried when required by
/// `policy`; this is empty under [`Policy::PreferNative`].
pub fn uncarried(policy: Policy, enabled: &[(&'static str, bool)]) -> Vec<&'static str> {
    match policy {
        Policy::PreferNative => Vec::new(),
        Policy::PortableOnly => enabled
            .iter()
            .filter_map(|(name, carried)| (!carried).then_some(*name))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::uncarried;
    use crate::Policy;

    #[test]
    fn portable_policy_requires_each_enabled_portable_realization() {
        assert_eq!(
            uncarried(Policy::PortableOnly, &[("barcode", true), ("text", false)]),
            ["text"]
        );
        assert_eq!(
            uncarried(Policy::PreferNative, &[("text", false)]),
            Vec::<&str>::new()
        );
    }
}
