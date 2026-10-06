//! Apple realization: `VisionKit`'s `DataScannerViewController`, presented
//! from the app's key window scene like the dialog pickers.
//!
//! The bridge functions live in `Scanner.swift`, compiled into the crate by
//! `build.rs`. The Swift side hops onto the main queue for presentation and
//! answers through `on_scan_result`, so nothing blocks: the callback
//! completes the awaiting [`crate::CodeScanner::scan`] through a oneshot.

use crate::{Barcode, Payload, Point, Quad, Symbology, VisionError};
use bytes::Bytes;
use enumset::EnumSet;
use futures::channel::oneshot;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

type ScanCallback = oneshot::Sender<Result<Option<Barcode>, VisionError>>;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn callbacks() -> &'static Mutex<HashMap<u64, ScanCallback>> {
    static LOCK: OnceLock<Mutex<HashMap<u64, ScanCallback>>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(HashMap::new()))
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
        Symbology::Itf => "itf",
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
        "itf" => Symbology::Itf,
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

fn quad_from_csv(csv: &str) -> Result<Quad, VisionError> {
    let malformed =
        || VisionError::Platform(format!("the scanner returned malformed bounds {csv:?}"));
    let parts: Vec<f32> = csv
        .split(',')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .map_err(|_| malformed())?;
    let [x1, y1, x2, y2, x3, y3, x4, y4]: [f32; 8] = parts.try_into().map_err(|_| malformed())?;
    Ok(Quad([
        Point { x: x1, y: y1 },
        Point { x: x2, y: y2 },
        Point { x: x3, y: y3 },
        Point { x: x4, y: y4 },
    ]))
}

#[swift_bridge::bridge]
mod ffi {
    extern "Swift" {
        fn scanner_supported_bridge() -> bool;
        fn scan_bridge(symbologies_csv: &str, cb_id: u64);
    }

    extern "Rust" {
        fn on_scan_result(
            cb_id: u64,
            payload: Option<String>,
            symbology: Option<String>,
            bounds: Option<String>,
            error: Option<String>,
        );
    }
}

fn on_scan_result(
    cb_id: u64,
    payload: Option<String>,
    symbology: Option<String>,
    bounds: Option<String>,
    error: Option<String>,
) {
    let tx = callbacks()
        .lock()
        .unwrap_or_else(|error| panic!("waterkit-vision: scan callback map lock poisoned: {error}"))
        .remove(&cb_id)
        .unwrap_or_else(|| panic!("waterkit-vision: unknown scan callback id in result: {cb_id}"));

    let result = match (error, payload) {
        (Some(message), _) => Err(VisionError::Platform(message)),
        (None, Some(payload)) => symbology
            .ok_or_else(|| {
                VisionError::Platform(
                    "the scanner returned a barcode without its symbology".to_owned(),
                )
            })
            .and_then(|id| symbology_from_id(&id))
            .and_then(|symbology| {
                bounds
                    .map(|csv| quad_from_csv(&csv))
                    .transpose()
                    .map(|quad| Barcode {
                        symbology,
                        payload: Payload {
                            bytes: Bytes::from(payload),
                        },
                        bounds: quad,
                    })
            })
            .map(Some),
        (None, None) => Ok(None),
    };
    let _ = tx.send(result);
}

/// Whether `DataScannerViewController` is supported on this device — false
/// on the simulator and on hardware too old for `VisionKit` scanning.
///
/// Camera authorization is a permission state, not device support: a denied
/// camera surfaces as a [`VisionError::Platform`] failure at scan time.
pub fn scanner_available() -> bool {
    ffi::scanner_supported_bridge()
}

/// Presents `DataScannerViewController` and resolves to the scanned barcode.
///
/// # Errors
///
/// Returns [`VisionError::Unsupported`] when the device does not support the
/// data scanner and [`VisionError::Platform`] when presentation or scanning
/// fails.
pub async fn scan(symbologies: EnumSet<Symbology>) -> Result<Option<Barcode>, VisionError> {
    if !scanner_available() {
        return Err(VisionError::Unsupported(
            "this device does not support VisionKit's DataScannerViewController".to_owned(),
        ));
    }
    let (tx, rx) = oneshot::channel();
    let cb_id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    callbacks()
        .lock()
        .map_err(|_| VisionError::Platform("scan callback map lock poisoned".to_owned()))?
        .insert(cb_id, tx);

    let symbologies_csv = symbologies
        .iter()
        .map(symbology_id)
        .collect::<Vec<_>>()
        .join(",");
    ffi::scan_bridge(&symbologies_csv, cb_id);

    rx.await
        .map_err(|_| VisionError::Platform("scan result channel closed".to_owned()))?
}
