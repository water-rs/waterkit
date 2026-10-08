//! Apple realization: `VisionKit`'s `VNDocumentCameraViewController`,
//! presented from the app's key window scene like the code scanner.
//!
//! The bridge functions live in `DocumentScanner.swift`, compiled into the
//! crate by `build.rs`. The Swift side hops onto the main queue for
//! presentation, encodes each page of the `VNDocumentCameraScan` as a JPEG
//! off the main actor and answers through `on_document_scan_result`, so
//! nothing blocks: the callback completes the awaiting
//! [`crate::DocumentScanner::scan`] through a oneshot. Pages cross the
//! bridge as a JSON array of base64-encoded JPEGs, the format every bridge
//! result in this crate crosses in.

use crate::document_scanner::{DocumentScannerOptions, pages_from_base64_json};
use crate::{Image, VisionError};
use futures::channel::oneshot;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

type ScanCallback = oneshot::Sender<Result<Option<Vec<Image>>, VisionError>>;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn callbacks() -> &'static Mutex<HashMap<u64, ScanCallback>> {
    static LOCK: OnceLock<Mutex<HashMap<u64, ScanCallback>>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(HashMap::new()))
}

#[swift_bridge::bridge]
mod ffi {
    extern "Swift" {
        fn document_scanner_supported_bridge() -> bool;
        fn scan_document_bridge(cb_id: u64);
    }

    extern "Rust" {
        // `pages_json` is a JSON array of base64-encoded JPEG pages.
        fn on_document_scan_result(
            cb_id: u64,
            pages_json: Option<String>,
            error: Option<String>,
        );
    }
}

fn on_document_scan_result(cb_id: u64, pages_json: Option<String>, error: Option<String>) {
    let tx = callbacks()
        .lock()
        .unwrap_or_else(|error| {
            panic!("waterkit-vision: document scan callback map lock poisoned: {error}")
        })
        .remove(&cb_id)
        .unwrap_or_else(|| {
            panic!("waterkit-vision: unknown document scan callback id in result: {cb_id}")
        });

    let result = match (error, pages_json) {
        (Some(message), _) => Err(VisionError::Platform(message)),
        (None, Some(json)) => pages_from_base64_json(&json).map(Some),
        (None, None) => Ok(None),
    };
    let _ = tx.send(result);
}

/// Whether `VNDocumentCameraViewController` is supported on this device —
/// false on the simulator and on hardware without a camera for document
/// scanning.
pub fn document_scanner_available() -> bool {
    ffi::document_scanner_supported_bridge()
}

/// `VNDocumentCameraViewController` has neither a page limit nor a gallery
/// import.
pub const fn document_scanner_options() -> DocumentScannerOptions {
    DocumentScannerOptions {
        page_limit: false,
        gallery_import: false,
    }
}

/// Presents `VNDocumentCameraViewController` and resolves to the scanned
/// pages.
///
/// # Errors
///
/// Returns [`VisionError::Unsupported`] when the device does not support the
/// document camera and [`VisionError::Platform`] when presentation or page
/// delivery fails.
pub async fn scan_document(
    _page_limit: Option<u16>,
    _gallery_import: bool,
) -> Result<Option<Vec<Image>>, VisionError> {
    if !document_scanner_available() {
        return Err(VisionError::Unsupported(
            "this device does not support VisionKit's VNDocumentCameraViewController".to_owned(),
        ));
    }
    let (tx, rx) = oneshot::channel();
    let cb_id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    callbacks()
        .lock()
        .map_err(|_| VisionError::Platform("document scan callback map lock poisoned".to_owned()))?
        .insert(cb_id, tx);

    ffi::scan_document_bridge(cb_id);

    rx.await
        .map_err(|_| VisionError::Platform("document scan result channel closed".to_owned()))?
}
