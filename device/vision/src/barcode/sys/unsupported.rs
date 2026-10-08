//! No native barcode realization exists on this platform.

use enumset::EnumSet;

use crate::{
    Barcode, Symbology, VisionError,
    barcode::{BarcodePlan, DetectBarcodes},
    sealed::{Offer, Pass},
};

/// This platform has no native barcode realization.
pub const fn native_symbologies() -> EnumSet<Symbology> {
    EnumSet::empty()
}

/// This platform has no native barcode realization.
pub const fn offer(_request: &DetectBarcodes) -> Offer {
    Offer::Absent
}

/// Native barcode detection is never selected here; `offer` is always
/// [`Offer::Absent`].
#[expect(
    clippy::unused_async,
    reason = "keeps the signature every platform's native realization shares"
)]
pub async fn prepare(_plan: &BarcodePlan) -> Result<(), VisionError> {
    unreachable!("a platform without a native barcode realization never selects it")
}

/// Native barcode detection is never selected here; `offer` is always
/// [`Offer::Absent`].
#[expect(
    clippy::unused_async,
    reason = "keeps the signature every platform's native realization shares"
)]
#[cfg_attr(
    target_arch = "wasm32",
    expect(
        clippy::future_not_send,
        reason = "on wasm32 wgpu devices, queues and textures are not `Send`, so neither is a future holding them"
    )
)]
pub async fn detect(
    _pass: &mut Pass<'_>,
    _plan: &BarcodePlan,
) -> Result<Vec<Barcode>, VisionError> {
    unreachable!("a platform without a native barcode realization never selects it")
}
