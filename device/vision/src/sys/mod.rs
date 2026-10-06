//! Platform dispatch for native and portable realizations.
//!
//! `native` names the operating system's own implementation when the device
//! has one. Portable realizations are application-carried implementations
//! selected by the `portable-*` features (#130, #132); none exists yet, so
//! every portable offer is [`Offer::Absent`].

#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple;

#[cfg(all(any(target_os = "ios", target_os = "macos"), feature = "barcode"))]
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
}

/// The portable barcode realization is application-carried code that does
/// not exist yet; its offer is always absent.
#[cfg(feature = "barcode")]
pub const fn portable_barcodes_offer() -> crate::sealed::Offer {
    crate::sealed::Offer::Absent
}
