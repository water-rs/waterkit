//! Barcode detection request and its result types.
//!
//! [`DetectBarcodes`] asks for every barcode, of the requested symbologies,
//! readable in the image. Its bounds answer where in the image each barcode
//! sits, normalized into the upright image's frame like every request's
//! [`Quad`].

use enumset::EnumSet;

use crate::{
    Quad, VisionError,
    sealed::{Context, Sealed},
    symbology::Symbology,
    sys,
};

/// One barcode found in the image.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Barcode {
    pub(crate) symbology: Symbology,
    pub(crate) payload: Payload,
    pub(crate) bounds: Quad,
}

impl Barcode {
    /// The barcode's symbology.
    #[must_use]
    pub const fn symbology(&self) -> Symbology {
        self.symbology
    }

    /// What the barcode encodes.
    #[must_use]
    pub const fn payload(&self) -> &Payload {
        &self.payload
    }

    /// Where in the image the barcode sits, normalized into the upright
    /// image's frame.
    #[must_use]
    pub const fn bounds(&self) -> Quad {
        self.bounds
    }
}

/// The raw bytes a barcode encodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payload {
    pub(crate) bytes: bytes::Bytes,
}

impl Payload {
    /// The raw payload bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The payload decoded as UTF-8 text, when the bytes are valid UTF-8.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        std::str::from_utf8(&self.bytes).ok()
    }
}

impl AsRef<[u8]> for Payload {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

/// Detect the barcodes readable in an image.
///
/// `symbologies` selects the formats to look for; the union of every
/// [`Symbology`] the crate names asks for all of them. Planning checks the
/// chosen realization covers the set, so the request never lands on an engine
/// that cannot read part of it.
#[derive(Debug)]
pub struct DetectBarcodes {
    symbologies: EnumSet<Symbology>,
}

impl DetectBarcodes {
    /// A request for `symbologies`.
    pub fn new(symbologies: impl Into<EnumSet<Symbology>>) -> Self {
        Self {
            symbologies: symbologies.into(),
        }
    }
}

impl Sealed for DetectBarcodes {
    type Plan = sys::BarcodePlan;

    fn plan(&self, context: Context<'_>) -> Result<Self::Plan, VisionError> {
        sys::plan_barcodes(context, self.symbologies)
    }
}

impl crate::Request for DetectBarcodes {
    type Output = Vec<Barcode>;
}
