//! Android realizations of the one-shot system scanners: the Google code
//! scanner ([`scanner`]) and the ML Kit document scanner
//! ([`document_scanner`]), both modules of Google Play services.
//!
//! Each capability ships a Kotlin helper (`ScannerHelper`,
//! `DocumentScannerHelper`) compiled into the application's classpath by the
//! packager together with the thin Play-services client the crate declares;
//! the scanning screens themselves are Play services' modules, so the app
//! needs no camera permission. Helpers answer over JNI exports and this side
//! completes the awaiting `scan` through a oneshot, so nothing blocks.

use waterkit_build::AndroidError;

use crate::VisionError;

#[cfg(feature = "document-scanner")]
mod document_scanner;
#[cfg(feature = "scanner")]
mod scanner;

#[cfg(feature = "document-scanner")]
pub use document_scanner::{document_scanner_available, document_scanner_options, scan_document};
#[cfg(feature = "scanner")]
pub use scanner::{scan, scanner_available, scanner_symbologies};

impl From<AndroidError> for VisionError {
    fn from(error: AndroidError) -> Self {
        Self::Platform(error.to_string())
    }
}
