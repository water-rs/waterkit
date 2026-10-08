//! No system scanner exists on this platform.
//!
//! The availability probes report each scanner unavailable, the expressible
//! symbology set and option support are empty and `scan` is an error: there
//! is no system realization to fall back to, and the portable fallback UI is
//! `WaterUI`'s, not this crate's.

use core::future::Future;

#[cfg(feature = "scanner")]
use enumset::EnumSet;

use crate::VisionError;
#[cfg(feature = "document-scanner")]
use crate::document_scanner::DocumentScannerOptions;
#[cfg(feature = "scanner")]
use crate::{ScannedCode, Symbology};

#[cfg(feature = "scanner")]
pub const fn scanner_available() -> bool {
    false
}

#[cfg(feature = "scanner")]
pub const fn scanner_symbologies() -> EnumSet<Symbology> {
    EnumSet::empty()
}

#[cfg(feature = "scanner")]
pub fn scan(
    _symbologies: EnumSet<Symbology>,
) -> impl Future<Output = Result<Option<ScannedCode>, VisionError>> + Send {
    core::future::ready(Err(VisionError::Unsupported(
        "this platform has no system code scanner".to_owned(),
    )))
}

#[cfg(feature = "document-scanner")]
pub const fn document_scanner_available() -> bool {
    false
}

#[cfg(feature = "document-scanner")]
pub const fn document_scanner_options() -> DocumentScannerOptions {
    DocumentScannerOptions {
        page_limit: false,
        gallery_import: false,
    }
}

#[cfg(feature = "document-scanner")]
// No `Send` bound: on wasm32 an `Image` holds wgpu handles that are not
// `Send`, and the returned future cannot promise what the value cannot
// provide. On native targets the future is `Send` regardless.
#[cfg_attr(
    target_arch = "wasm32",
    expect(
        clippy::future_not_send,
        reason = "on wasm32 wgpu devices, queues and textures are not `Send`, so neither is a future returning their `Image`s"
    )
)]
pub fn scan_document(
    _page_limit: Option<u16>,
    _gallery_import: bool,
) -> impl Future<Output = Result<Option<Vec<crate::Image>>, VisionError>> {
    core::future::ready(Err(VisionError::Unsupported(
        "this platform has no system document scanner".to_owned(),
    )))
}
