//! Platforms without a native vision realization report every capability
//! absent and refuse every request at planning.

use crate::VisionCapabilities;
#[cfg(any(feature = "barcode", feature = "text"))]
use crate::{Portable, RealizationSet};

/// No native vision realization exists on this platform.
pub fn capabilities() -> std::future::Ready<VisionCapabilities> {
    std::future::ready(VisionCapabilities {
        #[cfg(feature = "barcode")]
        barcodes: RealizationSet {
            native: enumset::EnumSet::empty(),
            portable: Portable::Absent,
        },
        #[cfg(feature = "text")]
        text: RealizationSet {
            native: Vec::new(),
            portable: Portable::Absent,
        },
    })
}

#[cfg(feature = "barcode")]
mod barcodes {
    use enumset::EnumSet;

    use crate::{
        Barcode, DetectBarcodes, Symbology, VisionError,
        sealed::{Context, Offer, Pass, Plan},
    };

    /// Never constructed: planning always fails with
    /// [`VisionError::Unsupported`].
    #[derive(Debug)]
    pub enum BarcodePlan {}

    pub fn plan_barcodes(
        context: Context<'_>,
        _symbologies: EnumSet<Symbology>,
    ) -> Result<BarcodePlan, VisionError> {
        Err(context
            .select("detect-barcodes", &Offer::Absent, &Offer::Absent)
            .expect_err("absent realizations never select"))
    }

    impl Plan<DetectBarcodes> for BarcodePlan {
        async fn prepare(&self, _context: Context<'_>) -> Result<(), VisionError> {
            unreachable!("planning never produces a plan on this platform")
        }

        async fn run(self, _pass: &mut Pass<'_>) -> Result<Vec<Barcode>, VisionError> {
            match self {}
        }
    }
}

#[cfg(feature = "barcode")]
pub use barcodes::*;

#[cfg(feature = "text")]
mod text {
    use crate::{
        RecognizeText, TextLine, VisionError,
        sealed::{Context, Offer, Pass, Plan},
    };

    /// Never constructed: planning always fails with
    /// [`VisionError::Unsupported`].
    #[derive(Debug)]
    pub enum TextPlan {}

    /// Refuses the request: languages that resolve to no single served
    /// script fail first, then the request fails because no realization
    /// serves this platform.
    pub fn plan_text(
        context: Context<'_>,
        request: &RecognizeText,
    ) -> Result<TextPlan, VisionError> {
        request.script().map_err(|failed| {
            VisionError::Unsupported(format!(
                "recognize-text: languages [{}] resolve to no single served script",
                failed.join(", ")
            ))
        })?;
        Err(context
            .select("recognize-text", &Offer::Absent, &Offer::Absent)
            .expect_err("absent realizations never select"))
    }

    impl Plan<RecognizeText> for TextPlan {
        async fn prepare(&self, _context: Context<'_>) -> Result<(), VisionError> {
            unreachable!("planning never produces a plan on this platform")
        }

        async fn run(self, _pass: &mut Pass<'_>) -> Result<Vec<TextLine>, VisionError> {
            match self {}
        }
    }
}

#[cfg(feature = "text")]
pub use text::*;
