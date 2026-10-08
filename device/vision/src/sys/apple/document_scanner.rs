//! Apple realization: `VisionKit`'s `VNDocumentCameraViewController`,
//! presented from the app's key window scene like the code scanner.
//!
//! The bridge functions live in `DocumentScanner.swift`, compiled into the
//! crate by `build.rs`. The Swift side hops onto the main queue for
//! presentation, renders each page of the `VNDocumentCameraScan` into a
//! `CVPixelBuffer` off the main actor and answers through the
//! `DocumentScanReply` it was issued, so nothing blocks: the reply
//! completes the awaiting [`crate::DocumentScanner::scan`] through a
//! oneshot. Pages
//! cross the bridge as the addresses of `CVPixelBuffer`s the Swift side
//! retained for Rust to adopt, like the buffers `apple_vision` hands to
//! Vision request handlers.

use crate::document_scanner::{DocumentScannerOptions, pages_from_buffers};
use crate::{Image, VisionError};
use futures::channel::oneshot;

/// The reply the presented document camera carries back to Rust: Swift owns
/// the opaque object for the life of the presentation and calls
/// `document_scan_reply_complete` exactly once, consuming it.
struct DocumentScanReply {
    sender: oneshot::Sender<Result<Option<Vec<Image>>, VisionError>>,
}

#[swift_bridge::bridge]
mod ffi {
    extern "Swift" {
        fn document_scanner_supported_bridge() -> bool;
        fn scan_document_bridge(reply: DocumentScanReply);
    }

    extern "Rust" {
        type DocumentScanReply;

        // `pages` holds one retained `CVPixelBuffer` address per scanned
        // page; empty means the user cancelled. `reply` is consumed: the
        // answer is delivered exactly once.
        fn document_scan_reply_complete(
            reply: DocumentScanReply,
            pages: Vec<usize>,
            error: Option<String>,
        );
    }
}

impl DocumentScanReply {
    const fn new(sender: oneshot::Sender<Result<Option<Vec<Image>>, VisionError>>) -> Self {
        Self { sender }
    }
}

/// Answers the awaiting [`scan_document`].
fn document_scan_reply_complete(
    reply: DocumentScanReply,
    pages: Vec<usize>,
    error: Option<String>,
) {
    let result = match (error, pages.is_empty()) {
        (Some(message), _) => Err(VisionError::Platform(message)),
        (None, true) => Ok(None),
        (None, false) => pages_from_buffers(pages).map(Some),
    };
    let _ = reply.sender.send(result);
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

    ffi::scan_document_bridge(DocumentScanReply::new(tx));

    rx.await
        .map_err(|_| VisionError::Platform("document scan result channel closed".to_owned()))?
}
