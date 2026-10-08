//! Platform dispatch for native and portable realizations.
//!
//! `native` names the operating system's own implementation when the device
//! has one. Portable realizations are application-carried implementations
//! selected by the `portable-*` features (#130, #132); none exists yet, so
//! every portable offer is [`Offer::Absent`].
//!
//! The `scanner` capability has its own per-platform modules: the Google
//! code scanner on Android and `VisionKit`'s `DataScannerViewController` on
//! iOS — Mac Catalyst excluded, where `DataScannerViewController` is
//! unavailable — with no system scanner elsewhere. The `barcode` and `text`
//! capabilities' Apple realization lives in [`apple_vision`].

#[cfg(all(
    any(target_os = "ios", target_os = "macos"),
    any(feature = "barcode", feature = "text")
))]
pub mod apple_vision;

/// A retained `CVPixelBuffer` that may cross threads.
///
/// Core Foundation's reference counting is thread-safe, and nothing here
/// reads or writes the buffer's pixels on the CPU: it is only handed to
/// Vision, which samples it on the GPU.
#[cfg(all(any(target_os = "ios", target_os = "macos"), feature = "camera"))]
#[derive(Debug, Clone)]
pub struct PixelBuffer(pub objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer>);

// SAFETY: see the type's documentation; the buffer is only retained, released
// and handed to Vision, all thread-safe in Core Video. `Sync` comes with the
// same argument: readers only carry the reference.
#[cfg(all(any(target_os = "ios", target_os = "macos"), feature = "camera"))]
#[expect(
    clippy::non_send_fields_in_send_ty,
    reason = "the Core Foundation reference is exactly what this impl vouches for"
)]
unsafe impl Send for PixelBuffer {}

// SAFETY: as above.
#[cfg(all(any(target_os = "ios", target_os = "macos"), feature = "camera"))]
unsafe impl Sync for PixelBuffer {}

#[cfg(all(
    target_os = "android",
    any(feature = "scanner", feature = "barcode", feature = "text")
))]
pub mod android;
#[cfg(all(feature = "scanner", target_os = "ios", not(target_abi = "macabi")))]
mod apple;
#[cfg(all(
    feature = "scanner",
    not(any(
        target_os = "android",
        all(target_os = "ios", not(target_abi = "macabi"))
    ))
))]
mod unsupported;

#[cfg(all(feature = "scanner", target_os = "android"))]
pub use android::{scan, scanner_available, scanner_symbologies};
#[cfg(all(feature = "scanner", target_os = "ios", not(target_abi = "macabi")))]
pub use apple::{scan, scanner_available, scanner_symbologies};
#[cfg(all(
    feature = "scanner",
    not(any(
        target_os = "android",
        all(target_os = "ios", not(target_abi = "macabi"))
    ))
))]
pub use unsupported::{scan, scanner_available, scanner_symbologies};

#[cfg(all(any(target_os = "ios", target_os = "macos"), feature = "barcode"))]
pub use apple_vision as native;

/// On Android the native barcode realization is ML Kit's barcode engine,
/// delivered by Play services' modules.
#[cfg(all(target_os = "android", feature = "barcode"))]
pub use android::barcode as native;

#[cfg(not(any(target_os = "ios", target_os = "macos", target_os = "android")))]
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
    #[must_use]
    pub const fn supported_symbologies() -> enumset::EnumSet<crate::Symbology> {
        enumset::EnumSet::empty()
    }

    /// There is no native realization on this platform, so `select` can
    /// never resolve a native realization here.
    #[cfg(feature = "barcode")]
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
