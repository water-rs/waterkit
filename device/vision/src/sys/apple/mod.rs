//! Apple realizations of the one-shot system scanners, presented from the
//! app's key window scene like the dialog pickers.
//!
//! [`scanner`] drives `VisionKit`'s `DataScannerViewController` for codes and
//! [`document_scanner`] drives its `VNDocumentCameraViewController` for pages.
//! Each keeps a Swift bridge beside it (`Scanner.swift`,
//! `DocumentScanner.swift`), compiled into the crate by `build.rs`. The Swift
//! side hops onto the main queue for presentation and answers through the
//! `on_*` callbacks, so nothing blocks: each callback completes the awaiting
//! `scan` through a oneshot.

#[cfg(feature = "scanner")]
mod scanner;
#[cfg(feature = "document-scanner")]
mod document_scanner;

#[cfg(feature = "scanner")]
pub use scanner::{scan, scanner_available, scanner_symbologies};
#[cfg(feature = "document-scanner")]
pub use document_scanner::{document_scanner_available, document_scanner_options, scan_document};
