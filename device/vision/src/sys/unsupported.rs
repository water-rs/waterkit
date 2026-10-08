//! No system code scanner exists on this platform.
//!
//! [`scanner_available`] reports the scanner unavailable, the expressible
//! symbology set is empty and [`scan`] is an error: there is no system
//! realization to fall back to, and the portable fallback UI is `WaterUI`'s,
//! not this crate's.

use core::future::Future;

use enumset::EnumSet;

use crate::{ScannedCode, Symbology, VisionError};

pub const fn scanner_available() -> bool {
    false
}

pub const fn scanner_symbologies() -> EnumSet<Symbology> {
    EnumSet::empty()
}

pub fn scan(
    _symbologies: EnumSet<Symbology>,
) -> impl Future<Output = Result<Option<ScannedCode>, VisionError>> + Send {
    core::future::ready(Err(VisionError::Unsupported(
        "this platform has no system code scanner".to_owned(),
    )))
}
