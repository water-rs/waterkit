//! Apple realization: `VisionKit`'s `DataScannerViewController`, presented
//! from the app's key window scene like the dialog pickers.
//!
//! The bridge functions live in `Scanner.swift`, compiled into the crate by
//! `build.rs`. The Swift side hops onto the main queue for presentation and
//! answers through the `ScanReply` it was issued, so nothing blocks: the
//! reply completes the awaiting [`crate::CodeScanner::scan`] through a
//! oneshot.

use crate::{Payload, ScannedCode, Symbology, VisionError};
use bytes::Bytes;
use enumset::EnumSet;
use futures::channel::oneshot;

/// The reply the presented scanner carries back to Rust: Swift owns the
/// opaque object for the life of the presentation and calls
/// `scan_reply_complete` exactly once, consuming it.
struct ScanReply {
    sender: oneshot::Sender<Result<Option<ScannedCode>, VisionError>>,
}

/// The symbology id `Scanner.swift` maps onto `VNBarcodeSymbology`.
const fn symbology_id(symbology: Symbology) -> &'static str {
    match symbology {
        Symbology::Aztec => "aztec",
        Symbology::Codabar => "codabar",
        Symbology::Code39 => "code39",
        Symbology::Code93 => "code93",
        Symbology::Code128 => "code128",
        Symbology::DataMatrix => "datamatrix",
        Symbology::Ean8 => "ean8",
        Symbology::Ean13 => "ean13",
        Symbology::Gs1DataBar => "gs1databar",
        Symbology::Gs1DataBarExpanded => "gs1databarexpanded",
        Symbology::Gs1DataBarLimited => "gs1databarlimited",
        Symbology::Itf => "itf",
        Symbology::Itf14 => "itf14",
        Symbology::MicroPdf417 => "micropdf417",
        Symbology::MicroQr => "microqr",
        Symbology::MsiPlessey => "msiplessey",
        Symbology::Pdf417 => "pdf417",
        Symbology::Qr => "qr",
        Symbology::UpcA => "upca",
        Symbology::UpcE => "upce",
    }
}

fn symbology_from_id(id: &str) -> Result<Symbology, VisionError> {
    let symbology = match id {
        "aztec" => Symbology::Aztec,
        "codabar" => Symbology::Codabar,
        "code39" => Symbology::Code39,
        "code93" => Symbology::Code93,
        "code128" => Symbology::Code128,
        "datamatrix" => Symbology::DataMatrix,
        "ean8" => Symbology::Ean8,
        "ean13" => Symbology::Ean13,
        "gs1databar" => Symbology::Gs1DataBar,
        "gs1databarexpanded" => Symbology::Gs1DataBarExpanded,
        "gs1databarlimited" => Symbology::Gs1DataBarLimited,
        "itf" => Symbology::Itf,
        "itf14" => Symbology::Itf14,
        "micropdf417" => Symbology::MicroPdf417,
        "microqr" => Symbology::MicroQr,
        "msiplessey" => Symbology::MsiPlessey,
        "pdf417" => Symbology::Pdf417,
        "qr" => Symbology::Qr,
        "upca" => Symbology::UpcA,
        "upce" => Symbology::UpcE,
        other => {
            return Err(VisionError::Platform(format!(
                "the scanner returned an unknown symbology id {other}"
            )));
        }
    };
    Ok(symbology)
}

#[swift_bridge::bridge]
mod ffi {
    extern "Swift" {
        fn scanner_supported_bridge() -> bool;
        fn symbology_supported_bridge(id: &str) -> bool;
        fn scan_bridge(symbologies_csv: &str, reply: ScanReply);
    }

    extern "Rust" {
        type ScanReply;

        // `reply` is consumed: the answer is delivered exactly once.
        fn scan_reply_complete(
            reply: ScanReply,
            payload: Option<String>,
            symbology: Option<String>,
            error: Option<String>,
        );
    }
}

impl ScanReply {
    const fn new(sender: oneshot::Sender<Result<Option<ScannedCode>, VisionError>>) -> Self {
        Self { sender }
    }
}

/// Answers the awaiting [`scan`].
fn scan_reply_complete(
    reply: ScanReply,
    payload: Option<String>,
    symbology: Option<String>,
    error: Option<String>,
) {
    let result = match (error, payload) {
        (Some(message), _) => Err(VisionError::Platform(message)),
        (None, Some(payload)) => symbology
            .ok_or_else(|| {
                VisionError::Platform(
                    "the scanner returned a barcode without its symbology".to_owned(),
                )
            })
            .and_then(|id| symbology_from_id(&id))
            .map(|symbology| ScannedCode {
                symbology,
                payload: Payload {
                    bytes: Bytes::from(payload),
                },
            })
            .map(Some),
        (None, None) => Ok(None),
    };
    let _ = reply.sender.send(result);
}

/// Whether `DataScannerViewController` is supported on this device — false
/// on the simulator and on hardware too old for `VisionKit` scanning.
///
/// Camera authorization is a permission state, not device support: a denied
/// camera surfaces as a [`VisionError::Platform`] failure at scan time.
pub fn scanner_available() -> bool {
    ffi::scanner_supported_bridge()
}

/// The symbologies `DataScannerViewController` can restrict a scan to on
/// this device: `VisionKit` recognizes every [`Symbology`], with `UpcA`
/// expressed as EAN-13 (a UPC-A is an EAN-13 with a leading 0) and
/// `MsiPlessey` requiring iOS 17.
pub fn scanner_symbologies() -> EnumSet<Symbology> {
    EnumSet::all()
        .iter()
        .filter(|symbology| ffi::symbology_supported_bridge(symbology_id(*symbology)))
        .collect()
}

/// Presents `DataScannerViewController` and resolves to the scanned barcode.
///
/// # Errors
///
/// Returns [`VisionError::Unsupported`] when the device does not support the
/// data scanner and [`VisionError::Platform`] when presentation or scanning
/// fails.
pub async fn scan(symbologies: EnumSet<Symbology>) -> Result<Option<ScannedCode>, VisionError> {
    if !scanner_available() {
        return Err(VisionError::Unsupported(
            "this device does not support VisionKit's DataScannerViewController".to_owned(),
        ));
    }
    let (tx, rx) = oneshot::channel();

    let symbologies_csv = symbologies
        .iter()
        .map(symbology_id)
        .collect::<Vec<_>>()
        .join(",");
    ffi::scan_bridge(&symbologies_csv, ScanReply::new(tx));

    rx.await
        .map_err(|_| VisionError::Platform("scan result channel closed".to_owned()))?
}
