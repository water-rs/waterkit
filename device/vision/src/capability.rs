use crate::Policy;

/// Vision capabilities available to this build on this device.
///
/// Fields arrive with the capability features (barcodes, text, scanner); each
/// reports what the device's native realization serves exactly and what the
/// application carries portably.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisionCapabilities {
    /// The barcode symbologies this build serves.
    #[cfg(feature = "barcode")]
    pub barcodes: RealizationSet<enumset::EnumSet<crate::Symbology>>,
    /// The languages this build serves at every [`crate::RecognitionLevel`].
    ///
    /// A level can serve fewer languages than this set advertises; offers
    /// check the requested level's exact set.
    #[cfg(feature = "text")]
    pub text: RealizationSet<Vec<icu_locale_core::LanguageIdentifier>>,
}

/// Native and portable realizations of a capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealizationSet<T> {
    /// The native realization.
    pub native: T,
    /// The portable realization carried by the application.
    pub portable: Portable<T>,
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
        let any = false;
        #[cfg(feature = "barcode")]
        let any = any
            || !self.barcodes.native.is_empty()
            || !matches!(self.barcodes.portable, Portable::Absent);
        #[cfg(feature = "text")]
        let any =
            any || !self.text.native.is_empty() || !matches!(self.text.portable, Portable::Absent);
        any
    }
}

/// Every capability enabled by this build and whether its portable
/// realization is carried by the application.
///
/// Each second element becomes `cfg!(feature = "portable-*")` once the
/// portable realizations land (#130, #132).
pub const ENABLED: &[(&str, bool)] = &[
    #[cfg(feature = "barcode")]
    ("barcode", false),
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
