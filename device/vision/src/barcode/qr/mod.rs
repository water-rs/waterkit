//! QR Code Model 2 decoder (ISO/IEC 18004), versions 1–10.
//!
//! Pipeline per plausible finder triple: dimension estimate → version
//! information (v ≥ 7) → alignment-pattern search (v ≥ 2) →
//! perspective sampling → timing check → format information → codeword
//! extraction → Reed–Solomon correction → segment decode → quiet-zone
//! probe. Each failing stage surfaces a structured [`DecodeAttempt`];
//! geometry that never forms a plausible triple stays silent, matching
//! the linear decoders' behaviour on non-symbol content.

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::too_many_lines,
    clippy::missing_const_for_fn
)]

mod data;
mod detect;
mod gf;
mod tables;

use crate::frame::CpuFrame;
use crate::geometry::{Point, Quadrilateral};
use crate::image::BitImage;

use detect::Trio;
use tables::EcLevel;

use super::{
    Barcode, DecodeAttempt, DecodeReport, Evidence, RejectReason, ScanAxes, Stage, Symbology,
};

/// Highest version this decoder supports (dimension 57).
const MAX_VERSION: u8 = 10;

/// Decode QR symbols from `bits`, appending results and reject
/// diagnostics to `report`.
pub fn decode(bits: &BitImage, axes: ScanAxes, frame: &CpuFrame<'_>, report: &mut DecodeReport) {
    let (centers, lines) = detect::find_centers(bits, axes);
    if centers.len() < 3 {
        return;
    }
    for trio in detect::trios(&centers).iter().take(8) {
        match decode_trio(bits, trio, lines, frame) {
            Ok(barcode) => {
                if !is_duplicate(&barcode, &report.barcodes) {
                    report.barcodes.push(barcode);
                }
            }
            Err(reason) => report.attempts.push(DecodeAttempt {
                symbology: Symbology::QrCode,
                reason,
            }),
        }
    }
}

/// Same-symbol check: an accepted QR quad whose centre sits within a
/// quarter of the candidate's width is the same physical code.
fn is_duplicate(b: &Barcode, existing: &[Barcode]) -> bool {
    let c = b.quad.center();
    existing.iter().any(|e| {
        e.symbology == Symbology::QrCode
            && e.quad.center().distance(c) < b.quad.width().mul_add(0.25, 1.0)
    })
}

/// Decode one oriented finder triple end to end.
fn decode_trio(
    bits: &BitImage,
    trio: &Trio,
    lines: u32,
    frame: &CpuFrame<'_>,
) -> Result<Barcode, RejectReason> {
    let dim = detect::dimension_estimate(trio).ok_or(RejectReason::StructureMismatch {
        stage: Stage::Dimension,
    })?;
    let version = u8::try_from((dim - 17) / 4).map_err(|_| RejectReason::StructureMismatch {
        stage: Stage::Dimension,
    })?;

    // Estimated transform from the finder centres alone: accurate
    // enough to read the version-information and alignment regions.
    let h_est = if version >= 2 {
        Some(
            detect::transform(trio, dim, None).ok_or(RejectReason::StructureMismatch {
                stage: Stage::Dimension,
            })?,
        )
    } else {
        None
    };

    // Version information (versions ≥ 7).
    if version >= 7 {
        let (v1, v2) = detect::version_bits(bits, h_est.as_ref().unwrap(), dim);
        let decoded = tables::decode_version(v1).or_else(|| tables::decode_version(v2));
        match decoded {
            Some((v, _)) if v == version => {}
            _ => {
                return Err(RejectReason::StructureMismatch {
                    stage: Stage::VersionInfo,
                });
            }
        }
    }
    if version == 0 || version > MAX_VERSION {
        return Err(RejectReason::StructureMismatch {
            stage: Stage::Dimension,
        });
    }

    // Alignment pattern anchors the bottom-right corner for v ≥ 2.
    let alignment = if version >= 2 {
        Some(
            detect::locate_alignment(bits, h_est.as_ref().unwrap(), dim).ok_or(
                RejectReason::StructureMismatch {
                    stage: Stage::Alignment,
                },
            )?,
        )
    } else {
        None
    };

    let h = detect::transform(trio, dim, alignment).ok_or(RejectReason::StructureMismatch {
        stage: Stage::Dimension,
    })?;
    let matrix = detect::sample(bits, &h, dim).ok_or(RejectReason::StructureMismatch {
        stage: Stage::Dimension,
    })?;

    if detect::timing_error_rate(&matrix, dim) > 0.3 {
        return Err(RejectReason::StructureMismatch {
            stage: Stage::Timing,
        });
    }

    // Format information: try the top-left copy, then the
    // bottom-right/bottom-left copy.
    let (copy1, copy2) = detect::format_bits(&matrix, dim);
    let (fmt_bits, _) = tables::decode_format(copy1)
        .or_else(|| tables::decode_format(copy2))
        .ok_or(RejectReason::StructureMismatch {
            stage: Stage::FormatInfo,
        })?;
    let level = EcLevel::from_bits((fmt_bits >> 3) as u8);
    let mask = (fmt_bits & 0x07) as u8;

    let structure =
        tables::ec_structure(version, level).ok_or(RejectReason::StructureMismatch {
            stage: Stage::FormatInfo,
        })?;
    let function = detect::function_mask(dim, version);
    let codewords = data::read_codewords(&matrix, &function, dim, mask, structure.total());
    if codewords.len() < structure.total() {
        return Err(RejectReason::Truncated {
            stage: Stage::Codewords,
        });
    }
    let (data_bytes, corrected) = data::correct_codewords(&codewords, &structure)?;
    let (text, raw) = data::parse_segments(&data_bytes, version)?;

    if let Some((side, found)) = detect::quiet_zone_violation(bits, &h, dim) {
        return Err(RejectReason::QuietZone {
            side,
            found_modules: found,
            required_modules: 4.0,
        });
    }

    let corner = |x: f64, y: f64| {
        h.apply(x, y)
            .unwrap_or_else(|| Point::new(trio.tl.x, trio.tl.y))
    };
    let d = f64::from(dim);
    let quad = Quadrilateral::new(
        corner(-0.5, -0.5),
        corner(d - 0.5, -0.5),
        corner(d - 0.5, d - 0.5),
        corner(-0.5, d - 0.5),
    );

    let blocks = structure.groups.iter().map(|g| g.count).sum::<usize>();
    let ec_total = (structure.ec_len * blocks) as f32;
    let confidence = (0.95f32 - 0.5 * corrected as f32 / ec_total.max(1.0)).clamp(0.4, 0.99);

    let rows_matched = trio.tl.count + trio.tr.count + trio.bl.count;
    Ok(Barcode {
        text,
        raw,
        symbology: Symbology::QrCode,
        confidence,
        quad,
        timestamp: frame.timestamp(),
        evidence: Evidence {
            rows_matched,
            rows_scanned: lines,
        },
    })
}
