use crate::Policy;

#[cfg(feature = "barcode")]
use crate::Symbology;
#[cfg(feature = "barcode")]
use enumset::EnumSet;
#[cfg(feature = "text")]
use icu_locale::LanguageIdentifier;

/// Vision capabilities available to this build.
///
/// Fields arrive with the capability features (barcodes, text, scanner).
/// A `native` set is empty when the platform's realization is unavailable on
/// this device - for Android, when Play services or the ML Kit modules are
/// absent. A `portable` field is [`Portable::Absent`] unless a `portable-*`
/// feature carries the realization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisionCapabilities {
    /// The symbologies each realization serves. An empty `native` set means
    /// no native barcode realization is available.
    #[cfg(feature = "barcode")]
    pub barcodes: RealizationSet<EnumSet<Symbology>>,
    /// The scripts each realization serves, as `und-<Script>` language
    /// identifiers (`und-Latn`, `und-Hani`, `und-Deva`, `und-Jpan`,
    /// `und-Kore`). An empty `native` list means no native text realization
    /// is available.
    #[cfg(feature = "text")]
    pub text: RealizationSet<Vec<LanguageIdentifier>>,
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
    /// Returns whether any capability has a realization on this device.
    fn available(&self) -> bool {
        let barcodes = {
            #[cfg(feature = "barcode")]
            {
                !self.barcodes.native.is_empty() || self.barcodes.portable != Portable::Absent
            }
            #[cfg(not(feature = "barcode"))]
            {
                false
            }
        };
        let text = {
            #[cfg(feature = "text")]
            {
                !self.text.native.is_empty() || self.text.portable != Portable::Absent
            }
            #[cfg(not(feature = "text"))]
            {
                false
            }
        };
        barcodes || text
    }
}

/// Every capability enabled by this build and whether its portable
/// realization is carried by the application. No `portable-*` feature exists
/// yet, so every entry reports `false`.
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
