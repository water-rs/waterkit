use bytes::Bytes;
#[cfg(feature = "barcode")]
use enumset::EnumSet;

use crate::{Quad, Symbology};
#[cfg(feature = "barcode")]
use crate::{
    VisionError,
    sealed::{Context, Offer, Pass, Plan, Realization, Sealed},
};

#[cfg(feature = "barcode")]
mod sys;

#[cfg(feature = "barcode")]
pub use sys::native_symbologies;

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

/// A barcode detection request over one image.
///
/// `symbologies` selects the formats to look for; the union of every
/// [`Symbology`] the crate names asks for all of them. Selection checks the
/// serving realization covers the set, so the request never lands on an
/// engine that cannot read part of it: a symbology ML Kit's barcode engine
/// cannot express declines the native offer and fails with
/// [`VisionError::Unsupported`] naming it when no portable realization is
/// carried.
#[cfg(feature = "barcode")]
#[derive(Debug)]
pub struct DetectBarcodes {
    symbologies: EnumSet<Symbology>,
}

#[cfg(feature = "barcode")]
impl DetectBarcodes {
    /// A request for `symbologies`.
    ///
    /// # Panics
    ///
    /// Panics when `symbologies` is empty: a request restricted to nothing
    /// can never produce a barcode.
    pub fn new(symbologies: impl Into<EnumSet<Symbology>>) -> Self {
        let symbologies = symbologies.into();
        assert!(
            !symbologies.is_empty(),
            "DetectBarcodes requires at least one symbology"
        );
        Self { symbologies }
    }
}

#[cfg(feature = "barcode")]
impl crate::Request for DetectBarcodes {
    type Output = Vec<Barcode>;
}

#[cfg(feature = "barcode")]
impl Sealed for DetectBarcodes {
    type Plan = BarcodePlan;

    fn plan(&self, context: Context<'_>) -> Result<Self::Plan, VisionError> {
        Ok(BarcodePlan {
            symbologies: self.symbologies,
            realization: context.select("barcode", &sys::offer(self), &Offer::Absent)?,
        })
    }
}

/// A barcode request's selected realization.
///
/// Public only because the sealed [`crate::Request`] contract names it;
/// realization code constructs it.
#[cfg(feature = "barcode")]
#[doc(hidden)]
#[derive(Debug)]
#[cfg_attr(
    not(target_os = "android"),
    expect(
        dead_code,
        reason = "only a native realization reads the symbology set, and this platform has none yet"
    )
)]
pub struct BarcodePlan {
    symbologies: EnumSet<Symbology>,
    realization: Realization,
}

#[cfg(feature = "barcode")]
impl Plan<DetectBarcodes> for BarcodePlan {
    #[cfg_attr(
        target_arch = "wasm32",
        expect(
            clippy::future_not_send,
            reason = "on wasm32 wgpu devices, queues and textures are not `Send`, so neither is a future holding them"
        )
    )]
    async fn prepare(&self, _context: Context<'_>) -> Result<(), VisionError> {
        match self.realization {
            Realization::Native => sys::prepare(self).await,
            Realization::Portable => {
                unreachable!("no portable barcode realization exists yet")
            }
        }
    }

    #[cfg_attr(
        target_arch = "wasm32",
        expect(
            clippy::future_not_send,
            reason = "on wasm32 wgpu devices, queues and textures are not `Send`, so neither is a future holding them"
        )
    )]
    async fn run(self, pass: &mut Pass<'_>) -> Result<Vec<Barcode>, VisionError> {
        match self.realization {
            Realization::Native => sys::detect(pass, &self).await,
            Realization::Portable => {
                unreachable!("no portable barcode realization exists yet")
            }
        }
    }
}
