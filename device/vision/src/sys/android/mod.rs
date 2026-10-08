//! Android vision realizations over Play services.
//!
//! `waterkit.vision.ScannerHelper` backs the one-shot code scanner
//! ([`CodeScanner`], `feature = "scanner"`): it is compiled into the
//! application's classpath by the packager together with the thin
//! `play-services-code-scanner` client this crate declares, and the scanning
//! screen itself is Play services' module, so the app needs no camera
//! permission.
//!
//! `waterkit.vision.{MlKitInput,BarcodeHelper,TextHelper}` back the
//! `barcode` and `text` requests' native realization over the unbundled
//! `play-services-mlkit-*` clients ([`mlkit`], [`barcode`]). The format
//! table below is shared: `GmsBarcodeScanning` and ML Kit's barcode engine
//! read the same `Barcode.FORMAT_*` constants.
//!
//! [`CodeScanner`]: crate::CodeScanner

#[cfg(feature = "barcode")]
pub mod barcode;
#[cfg(any(feature = "barcode", feature = "text"))]
pub mod mlkit;
#[cfg(feature = "scanner")]
mod scanner;

#[cfg(any(feature = "scanner", feature = "barcode"))]
use crate::Symbology;
use crate::VisionError;
#[cfg(any(feature = "scanner", feature = "barcode"))]
use enumset::EnumSet;
use jni::objects::{JValue, JValueOwned};
use jni::{jni_sig, jni_str};
use waterkit_build::{
    AndroidError, DexHelper, describe_jni_error, dex_helper, with_android_context,
};

#[cfg(feature = "scanner")]
pub use scanner::{scan, scanner_available, scanner_symbologies};

/// `waterkit.vision.PlayServices`, the helper every Android feature stage —
/// the single Play services probe the scanner, barcode and text
/// realizations share.
static PLAY_SERVICES: DexHelper = dex_helper!("waterkit.vision.PlayServices");

/// Whether Google Play services is usable on this device, probed on each
/// call — availability can change under the process, so nothing is cached.
pub fn play_services() -> Result<bool, VisionError> {
    with_android_context(|env, context| {
        let class = PLAY_SERVICES.class(env, context)?;
        env.call_static_method(
            class,
            jni_str!("hasGooglePlayServices"),
            jni_sig!("(Landroid/content/Context;)Z"),
            &[JValue::Object(context)],
        )
        .and_then(JValueOwned::z)
        .map_err(|error| {
            VisionError::Platform(format!(
                "probe Play services: {}",
                describe_jni_error(env, error)
            ))
        })
    })
}

impl From<AndroidError> for VisionError {
    fn from(error: AndroidError) -> Self {
        Self::Platform(error.to_string())
    }
}

/// The symbologies `GmsBarcodeScanning` can restrict a scan to and ML Kit's
/// barcode engine can read — the constants
/// `com.google.mlkit.vision.barcode.common.Barcode#FORMAT_*` defines.
/// `Itf14` is absent: the ITF format accepts any interleaved-2-of-5 length,
/// so it cannot restrict to the 14-digit symbology.
#[cfg(any(feature = "scanner", feature = "barcode"))]
const FORMATS: [(Symbology, i32); 13] = [
    (Symbology::Aztec, 4096),
    (Symbology::Codabar, 8),
    (Symbology::Code39, 2),
    (Symbology::Code93, 4),
    (Symbology::Code128, 1),
    (Symbology::DataMatrix, 16),
    (Symbology::Ean8, 64),
    (Symbology::Ean13, 32),
    (Symbology::Itf, 128),
    (Symbology::Pdf417, 2048),
    (Symbology::Qr, 256),
    (Symbology::UpcA, 512),
    (Symbology::UpcE, 1024),
];

/// The symbologies `Barcode.FORMAT_*` expresses — what the code scanner can
/// restrict to and what the barcode engine can read.
#[cfg(any(feature = "scanner", feature = "barcode"))]
pub fn served_symbologies() -> EnumSet<Symbology> {
    FORMATS.iter().map(|(symbology, _)| *symbology).collect()
}

/// The `Barcode.FORMAT_*` constant for `symbology`, when the platform's
/// engines express it.
#[cfg(any(feature = "scanner", feature = "barcode"))]
pub fn format_of(symbology: Symbology) -> Option<i32> {
    FORMATS
        .iter()
        .find_map(|(served, format)| (*served == symbology).then_some(*format))
}

/// The symbology a result's `format` names. The engines only report formats
/// they were asked for, so a miss means the engine or the wire disagrees.
#[cfg(any(feature = "scanner", feature = "barcode"))]
pub fn symbology_of(format: i32) -> Option<Symbology> {
    FORMATS
        .iter()
        .find_map(|(served, served_format)| (*served_format == format).then_some(*served))
}
