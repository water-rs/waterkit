//! Barcode detection request and results.

use enumset::EnumSet;

use crate::{
    Quad, Symbology,
    sealed::{Context, Pass, Plan, Sealed},
};

/// The symbologies the native barcode realization detects on this device.
///
/// On Apple this is `DetectBarcodesRequest.supportedSymbologies` mapped to
/// the shared [`Symbology`] vocabulary; it is empty on platforms without a
/// native detector.
#[must_use]
pub fn native_symbologies() -> EnumSet<Symbology> {
    crate::sys::native::supported_symbologies()
}

/// Detect barcodes in an image, in the given symbologies.
#[derive(Debug)]
pub struct DetectBarcodes {
    /// The symbologies to look for.
    symbologies: EnumSet<Symbology>,
}

impl DetectBarcodes {
    /// A request detecting the given symbologies.
    ///
    /// An empty set detects nothing; the serving realization fails symbologies
    /// it cannot express ahead of time, when the request is planned.
    #[must_use]
    pub fn new(symbologies: impl Into<EnumSet<Symbology>>) -> Self {
        Self {
            symbologies: symbologies.into(),
        }
    }

    /// The symbologies this request looks for.
    #[must_use]
    pub const fn symbologies(&self) -> EnumSet<Symbology> {
        self.symbologies
    }
}

/// A detected barcode.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct Barcode {
    /// The detected symbology.
    pub(crate) symbology: Symbology,
    /// The decoded payload.
    pub(crate) payload: Payload,
    /// The barcode's corners, in the image's upright space.
    pub(crate) bounds: Quad,
}

impl Barcode {
    /// The detected symbology.
    #[must_use]
    pub const fn symbology(&self) -> Symbology {
        self.symbology
    }

    /// The decoded payload.
    #[must_use]
    pub const fn payload(&self) -> &Payload {
        &self.payload
    }

    /// The barcode's corners, in the image's upright space.
    #[must_use]
    pub const fn bounds(&self) -> Quad {
        self.bounds
    }
}

/// A barcode's payload bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payload {
    /// The raw payload bytes.
    pub(crate) bytes: bytes::Bytes,
}

impl Payload {
    /// The raw payload bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The payload decoded as UTF-8 text, when it is valid UTF-8.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        std::str::from_utf8(&self.bytes).ok()
    }
}

impl AsRef<[u8]> for Payload {
    fn as_ref(&self) -> &[u8] {
        self.bytes()
    }
}

/// The detection plan a [`crate::Request`] for barcodes resolves to.
#[derive(Debug)]
pub struct BarcodePlan {
    /// The symbologies the request looks for.
    symbologies: EnumSet<Symbology>,
    /// Where detection runs.
    realization: crate::sealed::Realization,
}

impl crate::Request for DetectBarcodes {
    type Output = Vec<Barcode>;
}

impl Sealed for DetectBarcodes {
    type Plan = BarcodePlan;

    fn plan(&self, context: Context<'_>) -> Result<Self::Plan, crate::VisionError> {
        Ok(BarcodePlan {
            symbologies: self.symbologies,
            realization: context.select(
                "barcode detection",
                &crate::sys::native::barcodes_offer(self.symbologies),
                &crate::sys::portable_barcodes_offer(),
            )?,
        })
    }
}

impl Plan<DetectBarcodes> for BarcodePlan {
    async fn prepare(&self, _context: Context<'_>) -> Result<(), crate::VisionError> {
        // Everything the native realization needs is prepared lazily, on the
        // pass's shared image handler.
        Ok(())
    }

    async fn run(self, pass: &mut Pass<'_>) -> Result<Vec<Barcode>, crate::VisionError> {
        match self.realization {
            crate::sealed::Realization::Native => {
                crate::sys::native::detect_barcodes(pass, self.symbologies).await
            }
            crate::sealed::Realization::Portable => {
                unreachable!("portable barcode detection is not in this build")
            }
        }
    }
}
