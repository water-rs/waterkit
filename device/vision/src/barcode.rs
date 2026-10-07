//! Barcode detection request and results.

use bytes::Bytes;
#[cfg(feature = "barcode")]
use enumset::EnumSet;

#[cfg(feature = "barcode")]
use crate::sealed::{Context, Pass, Plan, Sealed};
use crate::{Quad, Symbology};

/// A decoded barcode with its symbology, payload and geometry — the output
/// of a `DetectBarcodes` request, where the bounds always exist.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Barcode {
    pub(crate) symbology: Symbology,
    pub(crate) payload: Payload,
    pub(crate) bounds: Quad,
}

impl Barcode {
    /// The symbology the payload was decoded as.
    #[must_use]
    pub const fn symbology(&self) -> Symbology {
        self.symbology
    }

    /// The decoded payload.
    #[must_use]
    pub const fn payload(&self) -> &Payload {
        &self.payload
    }

    /// The detected code's geometry, normalized to the analyzed image.
    #[must_use]
    pub const fn bounds(&self) -> Quad {
        self.bounds
    }
}

/// The decoded content of a barcode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payload {
    pub(crate) bytes: Bytes,
}

impl Payload {
    /// The decoded bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The payload as UTF-8 text, when the bytes are valid UTF-8.
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

/// The symbologies the native barcode realization detects on this device.
///
/// On Apple this is `DetectBarcodesRequest.supportedSymbologies` mapped to
/// the shared [`Symbology`] vocabulary; it is empty on platforms without a
/// native detector.
#[cfg(feature = "barcode")]
#[must_use]
pub fn native_symbologies() -> EnumSet<Symbology> {
    crate::sys::native::supported_symbologies()
}

/// Detect barcodes in an image, in the given symbologies.
#[cfg(feature = "barcode")]
#[derive(Debug)]
pub struct DetectBarcodes {
    /// The symbologies to look for.
    symbologies: EnumSet<Symbology>,
}

#[cfg(feature = "barcode")]
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

/// The detection plan a [`crate::Request`] for barcodes resolves to.
#[cfg(feature = "barcode")]
#[derive(Debug)]
pub struct BarcodePlan {
    /// The symbologies the request looks for.
    symbologies: EnumSet<Symbology>,
    /// Where detection runs.
    realization: crate::sealed::Realization,
}

#[cfg(feature = "barcode")]
impl crate::Request for DetectBarcodes {
    type Output = Vec<Barcode>;
}

#[cfg(feature = "barcode")]
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

#[cfg(feature = "barcode")]
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
