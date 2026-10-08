use crate::Policy;

/// Vision capabilities available to this build.
///
/// Fields arrive with the capability features (text, barcodes, scanner).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisionCapabilities {
    /// Text recognition: the languages each realization serves.
    ///
    /// On Windows `native` is `OcrEngine::AvailableRecognizerLanguages`
    /// exactly; it is empty on platforms without a native recognizer.
    #[cfg(feature = "text")]
    pub text: RealizationSet<Vec<icu_locale_core::LanguageIdentifier>>,

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
            any(not(feature = "text"), not(target_os = "windows")),
            any(
                not(feature = "scanner"),
                not(any(target_os = "ios", target_os = "android"))
            )
        ),
        expect(
            clippy::missing_const_for_fn,
            reason = "a native text recognizer's language probe or the scanner's device-support probe is a runtime call; elsewhere the capabilities are constant"
        )
    )]
    pub(crate) fn new() -> Self {
        Self {
            #[cfg(feature = "text")]
            text: RealizationSet {
                native: crate::text::native_languages(),
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
        #[cfg(feature = "text")]
        if self.text.available() {
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
/// The system code scanner is not a request served by [`Vision`]: it has no
/// portable realization to select, so it is absent here even when its feature
/// is enabled and [`Policy::PortableOnly`] does not constrain it.
///
/// [`Vision`]: crate::Vision
/// [`Policy::PortableOnly`]: crate::Policy::PortableOnly
pub const ENABLED: &[(&str, bool)] = &[
    #[cfg(feature = "text")]
    ("text", false),
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
