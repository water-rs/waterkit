//! No system code scanner exists on this platform.
//!
//! [`scanner_available`] reports the scanner unavailable and [`scan`] is
//! an error: there is no system realization to fall back to, and the
//! portable fallback UI is `WaterUI`'s, not this crate's.

use core::future::Future;

use enumset::EnumSet;

use crate::{Barcode, Symbology, VisionError};

pub const fn scanner_available() -> bool {
    false
}

pub fn scan(
    _symbologies: EnumSet<Symbology>,
) -> impl Future<Output = Result<Option<Barcode>, VisionError>> + Send {
    core::future::ready(Err(VisionError::Unsupported(
        "this platform has no system code scanner".to_owned(),
    )))
}
