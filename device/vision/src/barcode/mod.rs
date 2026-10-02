//! Barcode decoding engine, authored in-crate.
//!
//! The engine decodes a [`CpuFrame`] into zero or more [`Barcode`]
//! results. Supported symbologies are declared by [`Formats`]; requesting
//! an unsupported set yields [`VisionError::UnsupportedFormat`] at
//! construction so unsupported paths fail fast rather than returning
//! empty results.
//!
//! ## Determinism
//!
//! `decode` results are sorted by `(min_y, min_x)` of the source
//! quadrilateral — the same frame always produces the same order.
//!
//! ## Diagnostics
//!
//! [`BarcodeEngine::decode_report`] additionally returns one
//! [`DecodeAttempt`] per rejected candidate (start guard found but a later
//! stage failed), carrying a structured [`RejectReason`]. Clean rows and
//! non-symbol content produce no diagnostics.

mod ean;
mod qr;

use std::fmt;

use waterkit_core::Timestamp;

use crate::error::VisionError;
use crate::frame::CpuFrame;
use crate::geometry::{Point, Quadrilateral};
use crate::image::{self, BitImage, GrayImage};

/// Barcode symbology identifiers.
///
/// The full vocabulary is defined up front; only the symbologies exposed
/// through [`Formats`] can currently be produced by [`BarcodeEngine`].
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Symbology {
    /// EAN-13 (ISO/IEC 15420).
    Ean13,
    /// UPC-A, reported as EAN-13 with a leading `0` digit.
    UpcA,
    /// EAN-8.
    Ean8,
    /// UPC-E.
    UpcE,
    /// Code 128 (all subsets).
    Code128,
    /// Code 39.
    Code39,
    /// Interleaved 2 of 5.
    Itf,
    /// ITF-14.
    Itf14,
    /// QR Code Model 2.
    QrCode,
    /// Data Matrix.
    DataMatrix,
    /// Aztec Code.
    Aztec,
    /// PDF417.
    Pdf417,
}

/// Selectable symbology set. Only decodable formats are declared.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Formats(u32);

impl Formats {
    /// EAN-13.
    pub const EAN13: Self = Self(1 << 0);
    /// UPC-A. Decoded through the EAN-13 pipeline; results whose first
    /// digit is `0` are reported as [`Symbology::UpcA`].
    pub const UPCA: Self = Self(1 << 1);
    /// QR Code Model 2 (ISO/IEC 18004), versions 1-10.
    pub const QR: Self = Self(1 << 2);
    /// Every implemented format.
    pub const ALL: Self = Self(Self::EAN13.0 | Self::UPCA.0 | Self::QR.0);

    /// Empty set.
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Union of two sets.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether `other` is fully contained in this set.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    /// Whether the set selects nothing.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Debug for Formats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        f.write_str("Formats(")?;
        for (bits, name) in [
            (Self::EAN13.0, "EAN13"),
            (Self::UPCA.0, "UPCA"),
            (Self::QR.0, "QR"),
        ] {
            if self.0 & bits != 0 {
                if !first {
                    f.write_str(" | ")?;
                }
                f.write_str(name)?;
                first = false;
            }
        }
        if first {
            f.write_str("empty")?;
        }
        f.write_str(")")
    }
}

/// Scanline directions the decoder walks.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ScanAxes {
    /// Horizontal scanlines only.
    Horizontal,
    /// Vertical scanlines only (codes rotated 90°).
    Vertical,
    /// Both axes (default).
    #[default]
    Both,
}

/// Options controlling a single decode pass.
#[derive(Debug, Clone, Copy, Default)]
pub struct DecodeOptions {
    /// Also decode an inverted copy of the frame (light bars on dark).
    pub inverted: bool,
    /// Scanline axes to search.
    pub axes: ScanAxes,
}

/// One decoded barcode.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct Barcode {
    /// Decoded payload text (ASCII digits for linear codes).
    pub text: String,
    /// Raw payload bytes before text interpretation.
    pub raw: Vec<u8>,
    /// Symbology that produced this result.
    pub symbology: Symbology,
    /// Confidence in `[0, 1]`, derived from scanline evidence.
    pub confidence: f32,
    /// Source quadrilateral in frame pixel coordinates.
    pub quad: Quadrilateral,
    /// Frame timestamp of the decoded image.
    pub timestamp: Timestamp,
    /// Supporting evidence for `confidence`.
    pub evidence: Evidence,
}

/// Evidence behind a result — how many independent scanlines decoded the
/// same payload.
#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
pub struct Evidence {
    /// Number of scanlines that decoded this payload.
    pub rows_matched: u32,
    /// Total scanlines that intersected the symbol.
    pub rows_scanned: u32,
}

/// Why a candidate symbol was rejected.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum RejectReason {
    /// Quiet zone narrower than the spec minimum.
    QuietZone {
        /// Which side failed.
        side: QuietSide,
        /// Measured width in modules.
        found_modules: f32,
        /// Spec-required width in modules.
        required_modules: f32,
    },
    /// Guard pattern did not match expected structure.
    GuardMismatch {
        /// Where the mismatch occurred.
        stage: Stage,
    },
    /// The row ended mid-symbol.
    Truncated {
        /// Where decoding stopped.
        stage: Stage,
    },
    /// A digit group matched no pattern within tolerance.
    DigitUndecodable {
        /// Digit index in the payload (0-based).
        position: u8,
    },
    /// First-digit parity pattern matched no valid combination.
    FirstDigitParity {
        /// The observed L/G parity bits.
        parity: u8,
    },
    /// Digit checksum did not validate.
    ChecksumMismatch {
        /// Expected check digit.
        expected: u8,
        /// Encoded check digit.
        actual: u8,
    },
    /// Run structure inconsistent with the symbology.
    StructureMismatch {
        /// Where the mismatch occurred.
        stage: Stage,
    },
    /// Reed-Solomon correction failed on at least one block.
    ErrorCorrectionFailed {
        /// Number of blocks the symbol was split into.
        blocks: u8,
    },
    /// A segment-mode identifier with no decoder path (Kanji, Hanzi,
    /// or a reserved mode).
    ModeInvalid {
        /// Bit offset of the mode indicator in the stream.
        bit: u32,
        /// The 4-bit mode value.
        mode: u8,
    },
    /// Payload bytes failed interpretation (bad charset data or an
    /// out-of-range group value).
    DataUndecodable {
        /// Bit offset of the failing segment.
        bit: u32,
    },
}

/// Quiet-zone side for [`RejectReason::QuietZone`].
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuietSide {
    /// Left (or top for vertical scans) margin.
    Left,
    /// Right (or bottom) margin.
    Right,
    /// Top margin (matrix codes).
    Top,
    /// Bottom margin (matrix codes).
    Bottom,
}

/// Decode stage for stage-scoped rejections.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Start guard.
    StartGuard,
    /// Left digit group.
    LeftDigits,
    /// Middle guard.
    MiddleGuard,
    /// Right digit group.
    RightDigits,
    /// End guard.
    EndGuard,
    /// Symbol dimension estimation (matrix codes).
    Dimension,
    /// Alignment-pattern search.
    Alignment,
    /// Timing-pattern check.
    Timing,
    /// Format information decode.
    FormatInfo,
    /// Version information decode.
    VersionInfo,
    /// Codeword extraction.
    Codewords,
    /// Segment stream parsing.
    Segments,
}

/// One rejected decode candidate with its reason.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct DecodeAttempt {
    /// Symbology the candidate was being decoded as.
    pub symbology: Symbology,
    /// Why it was rejected.
    pub reason: RejectReason,
}

/// Full decode output: accepted symbols plus reject diagnostics.
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct DecodeReport {
    /// Decoded barcodes, ordered by source geometry.
    pub barcodes: Vec<Barcode>,
    /// Rejected candidates, in scan order.
    pub attempts: Vec<DecodeAttempt>,
}

/// Stateless decoding engine. Cloneable and `Send + Sync`.
#[derive(Debug, Clone)]
pub struct BarcodeEngine {
    formats: Formats,
    options: DecodeOptions,
}

impl BarcodeEngine {
    /// Engine decoding all implemented formats with default options.
    #[must_use]
    pub fn new() -> Self {
        Self::with_formats(Formats::ALL)
    }

    /// Engine restricted to `formats`.
    ///
    /// # Panics
    /// Panics if `formats` is empty.
    #[must_use]
    pub fn with_formats(formats: Formats) -> Self {
        assert!(!formats.is_empty(), "BarcodeEngine requires >= 1 format");
        Self {
            formats,
            options: DecodeOptions::default(),
        }
    }

    /// Builder-style override of decode options.
    #[must_use]
    pub const fn with_options(mut self, options: DecodeOptions) -> Self {
        self.options = options;
        self
    }

    /// The formats this engine decodes.
    #[must_use]
    pub const fn formats(&self) -> Formats {
        self.formats
    }

    /// Decode a CPU frame, returning barcodes only.
    ///
    /// # Errors
    /// Returns [`VisionError::InvalidFrame`] if the frame is degenerate
    /// (empty luma extraction).
    pub fn decode(&self, frame: &CpuFrame<'_>) -> Result<Vec<Barcode>, VisionError> {
        Ok(self.decode_report(frame)?.barcodes)
    }

    /// Decode a CPU frame, returning results plus reject diagnostics.
    ///
    /// # Errors
    /// Returns [`VisionError::InvalidFrame`] for a degenerate frame.
    pub fn decode_report(&self, frame: &CpuFrame<'_>) -> Result<DecodeReport, VisionError> {
        if frame.width() == 0 || frame.height() == 0 {
            return Err(VisionError::InvalidFrame("empty frame dimensions".into()));
        }
        let gray = GrayImage::from_cpu_frame(frame);
        let mut report = self.decode_gray(&gray, frame);
        if self.options.inverted {
            let bits = image::binarize(&gray);
            let inv = image::invert(&bits);
            let mut inv_report = DecodeReport::default();
            self.decode_bits(&inv, frame, &mut inv_report);
            // Merge: only keep inverted-only payloads not already found.
            let seen: Vec<Vec<u8>> = report.barcodes.iter().map(|b| b.raw.clone()).collect();
            for b in inv_report.barcodes {
                if !seen.contains(&b.raw) {
                    report.barcodes.push(b);
                }
            }
            report.attempts.extend(inv_report.attempts);
        }
        sort_barcodes(&mut report.barcodes);
        Ok(report)
    }

    fn decode_gray(&self, gray: &GrayImage, frame: &CpuFrame<'_>) -> DecodeReport {
        let bits = image::binarize(gray);
        let mut report = DecodeReport::default();
        self.decode_bits(&bits, frame, &mut report);
        report
    }

    /// Run all enabled decoders over `bits`, appending to `report`.
    fn decode_bits(&self, bits: &BitImage, frame: &CpuFrame<'_>, report: &mut DecodeReport) {
        if self.formats.contains(Formats::QR) {
            qr::decode(bits, self.options.axes, frame, report);
        }
        let axes = self.options.axes;
        let mut hits: Vec<(u32, ean::RowHit, bool)> = Vec::new(); // (line, hit, vertical)

        if matches!(axes, ScanAxes::Horizontal | ScanAxes::Both) {
            for y in 0..bits.height {
                let runs = bits.row_runs(y);
                for hit in ean::decode_runs(&runs, &mut report.attempts) {
                    hits.push((y, hit, false));
                }
            }
        }
        if matches!(axes, ScanAxes::Vertical | ScanAxes::Both) {
            for x in 0..bits.width {
                let runs = bits.column_runs(x);
                for hit in ean::decode_runs(&runs, &mut report.attempts) {
                    hits.push((x, hit, true));
                }
            }
        }

        // Group by digits + axis; merge spatially overlapping hits.
        for vertical in [false, true] {
            let axis_hits: Vec<(u32, ean::RowHit)> = hits
                .iter()
                .filter(|(_, _, v)| *v == vertical)
                .map(|(line, h, _)| (*line, h.clone()))
                .collect();
            for group in ean::group_hits(&axis_hits) {
                let sym = hit_symbology(&group.digits);
                if !self.formats.contains(sym.formats_bit()) {
                    continue;
                }
                report
                    .barcodes
                    .push(group_to_barcode(&group, sym, vertical, bits, frame));
            }
        }
    }
}

impl Default for BarcodeEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Map decoded digits to their symbology: a leading zero is the UPC-A
/// subset of EAN-13.
const fn hit_symbology(digits: &[u8; 13]) -> Symbology {
    if digits[0] == 0 {
        Symbology::UpcA
    } else {
        Symbology::Ean13
    }
}

impl Symbology {
    /// The [`Formats`] bit selecting results of this symbology.
    const fn formats_bit(self) -> Formats {
        match self {
            Self::Ean13 => Formats::EAN13,
            Self::UpcA => Formats::UPCA,
            Self::QrCode => Formats::QR,
            _ => Formats::empty(),
        }
    }
}

/// Build a [`Barcode`] from merged row hits.
fn group_to_barcode(
    group: &ean::HitGroup,
    symbology: Symbology,
    vertical: bool,
    bits: &BitImage,
    frame: &CpuFrame<'_>,
) -> Barcode {
    let (digits, x0, x1) = (group.digits, f64::from(group.x0), f64::from(group.x1));
    let l0 = f64::from(*group.rows.iter().min().unwrap_or(&0));
    let l1 = f64::from(*group.rows.iter().max().unwrap_or(&0));
    let quad = if vertical {
        // Column scans: the digit axis is y, rows[] holds column indices.
        Quadrilateral::new(
            Point::new(l0, x0),
            Point::new(l1, x0),
            Point::new(l1, x1),
            Point::new(l0, x1),
        )
    } else {
        Quadrilateral::new(
            Point::new(x0, l0),
            Point::new(x1, l0),
            Point::new(x1, l1),
            Point::new(x0, l1),
        )
    };
    let payload: &[u8] = if symbology == Symbology::UpcA {
        &digits[1..]
    } else {
        &digits[..]
    };
    let text: String = payload.iter().map(|d| (b'0' + d) as char).collect();
    let rows_matched = u32::try_from(group.rows.len()).unwrap_or(u32::MAX);
    let rows_scanned = if vertical { bits.width } else { bits.height };
    Barcode {
        raw: text.as_bytes().to_vec(),
        text,
        symbology,
        confidence: confidence(rows_matched),
        quad,
        timestamp: frame.timestamp(),
        evidence: Evidence {
            rows_matched,
            rows_scanned,
        },
    }
}

/// Confidence from cross-scanline agreement: one scanline is evidence of
/// a real symbol, three or more independent lines is strong.
fn confidence(rows_matched: u32) -> f32 {
    let n = f32::from(u16::try_from(rows_matched.min(4)).unwrap_or(4));
    0.15f32.mul_add(n, 0.4).min(1.0)
}

/// Deterministic order: top-to-bottom, then left-to-right.
fn sort_barcodes(v: &mut [Barcode]) {
    v.sort_by(|a, b| {
        a.quad
            .min_y()
            .total_cmp(&b.quad.min_y())
            .then_with(|| a.quad.min_x().total_cmp(&b.quad.min_x()))
    });
}
