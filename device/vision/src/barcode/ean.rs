//! EAN-13 / UPC-A row decoding.
//!
//! One row of a binarized image is described as a list of runs (see
//! [`crate::image::Run`]). A valid EAN-13 symbol on that row is:
//!
//! ```text
//! [quiet zone >= 11m][B W B][6 x 4-element digits][W B W B W][6 x 4][B W B][quiet >= 7m]
//! ```
//!
//! where `m` is the module width estimated from the start guard. Each
//! digit occupies 7 modules as four alternating runs; the first digit is
//! encoded implicitly by the L/G parity pattern of the left group.
//! Right-side digits use the R parity, whose run-length vector is the
//! same as the left group's — only the leading colour differs.
//!
//! UPC-A is the strict subset of EAN-13 whose first digit is `0`; such
//! results are reported with [`Symbology::UpcA`] and the 12-digit payload
//! (leading zero dropped).

use super::{DecodeAttempt, RejectReason};
use crate::image::Run;

/// Left-parity (L) digit patterns as `[white, black, white, black]`
/// run-lengths in modules, per ISO/IEC 15420.
const L_RUNS: [[u8; 4]; 10] = [
    [3, 2, 1, 1], // 0
    [2, 2, 2, 1], // 1
    [2, 1, 2, 2], // 2
    [1, 4, 1, 1], // 3
    [1, 1, 3, 2], // 4
    [1, 2, 3, 1], // 5
    [1, 1, 1, 4], // 6
    [1, 3, 1, 2], // 7
    [1, 2, 1, 3], // 8
    [3, 1, 1, 2], // 9
];

/// First-digit selection: the L/G parity pattern of the six left digits
/// (bit set = G parity), per ISO/IEC 15420 table.
const FIRST_DIGIT_PARITY: [u8; 10] = [
    0b00_0000, // 0: LLLLLL
    0b00_1011, // 1: LLGLGG
    0b00_1101, // 2: LLGGLG
    0b00_1110, // 3: LLGGGL
    0b01_0011, // 4: LGLLGG
    0b01_1001, // 5: LGGLLG
    0b01_1100, // 6: LGGGLL
    0b01_0101, // 7: LGLGLG
    0b01_0110, // 8: LGLGGL
    0b01_1010, // 9: LGGLGL
];

/// Required left quiet zone, in modules (ISO/IEC 15420).
const QUIET_LEFT_MODULES: f32 = 11.0;
/// Required right quiet zone, in modules.
const QUIET_RIGHT_MODULES: f32 = 7.0;
/// Slack applied to guard-run length comparison, relative to `m`.
const GUARD_TOLERANCE: f32 = 0.7;
/// Maximum summed run-length error allowed per digit, relative to `m`.
const DIGIT_TOLERANCE: f32 = 1.8;

/// A successfully decoded row hit, before cross-row merging.
#[derive(Debug, Clone)]
pub(crate) struct RowHit {
    /// First dark pixel of the start guard.
    pub(crate) x0: u32,
    /// One past the last dark pixel of the end guard.
    pub(crate) x1: u32,
    /// Thirteen decoded digits (EAN-13 form; first digit from parity).
    pub(crate) digits: [u8; 13],
}

/// Decode every EAN-13 candidate along one scanline's run list.
///
/// `diagnostics` receives one [`DecodeAttempt`] per candidate that matched
/// a start guard but failed a later stage — rows with no plausible start
/// guard produce no diagnostics (they are not candidate codes).
pub(crate) fn decode_runs(runs: &[Run], diagnostics: &mut Vec<DecodeAttempt>) -> Vec<RowHit> {
    let mut hits = Vec::new();
    let mut i = 0usize;
    while i + 3 <= runs.len() {
        // A start guard is a dark run preceded by quiet space.
        if !runs[i].dark {
            i += 1;
            continue;
        }
        match decode_at(runs, i) {
            Ok(hit) => {
                // Resume scanning after this symbol's end guard.
                let end_x = hit.x1;
                hits.push(hit);
                while i < runs.len() && runs[i].start < end_x {
                    i += 1;
                }
            }
            Err(Some(reason)) => {
                diagnostics.push(DecodeAttempt {
                    symbology: SymbologyRef::Ean13,
                    reason,
                });
                i += 1;
            }
            Err(None) => i += 1,
        }
    }
    hits
}

/// Symbology tag used in diagnostics (this decoder only produces EAN-13
/// candidates; UPC-A is resolved at merge time).
#[derive(Debug, Clone, Copy)]
pub(crate) enum SymbologyRef {
    Ean13,
}

/// Attempt a full symbol decode with the start guard beginning at `runs[i]`.
///
/// `Err(None)` means the candidate is not plausible enough to report
/// (guard shape mismatch); `Err(Some(reason))` is a real reject.
fn decode_at(runs: &[Run], i: usize) -> Result<RowHit, Option<RejectReason>> {
    // --- start guard: three runs [dark, light, dark] each ~1 module ---
    let g = &runs[i..i + 3];
    if !(g[0].dark && !g[1].dark && g[2].dark) {
        return Err(None);
    }
    let m = (g[0].len + g[1].len + g[2].len) as f32 / 3.0;
    if m < 1.0 || !guard_match(&[g[0].len, g[1].len, g[2].len], m) {
        return Err(None);
    }

    // --- left quiet zone: distance back to the previous dark run ---
    let quiet_left = if i >= 2 {
        // runs[i-1] is the light run before the guard; runs[i-2] dark.
        runs[i - 1].len
    } else {
        runs[i].start // quiet extends to the scanline edge
    };
    if (quiet_left as f32) < (QUIET_LEFT_MODULES - 1.0) * m {
        return Err(Some(RejectReason::QuietZone {
            side: QuietSide::Left,
            found_modules: quiet_left as f32 / m,
            required_modules: QUIET_LEFT_MODULES,
        }));
    }

    // --- six left digits (L/G parity) ---
    let mut pos = i + 3;
    let mut digits = [0u8; 13];
    let mut parity = 0u8;
    for d in 0..6 {
        if pos + 4 > runs.len() {
            return Err(Some(RejectReason::Truncated {
                stage: Stage::LeftDigits,
            }));
        }
        let group = &runs[pos..pos + 4];
        if group[0].dark {
            // Left digits start with a light run.
            return Err(Some(RejectReason::StructureMismatch {
                stage: Stage::LeftDigits,
            }));
        }
        match match_digit(group, m, Parity::Both) {
            Some((digit, is_g, _err)) => {
                digits[1 + d] = digit;
                if is_g {
                    parity |= 1 << (5 - d);
                }
            }
            None => {
                return Err(Some(RejectReason::DigitUndecodable {
                    position: 1 + d,
                }))
            }
        }
        pos += 4;
    }
    let Some(first) = FIRST_DIGIT_PARITY
        .iter()
        .position(|&p| p == parity)
        .map(|p| p as u8)
    else {
        return Err(Some(RejectReason::FirstDigitParity { parity }));
    };
    digits[0] = first;

    // --- middle guard: [light, dark, light, dark, light], 5 modules ---
    if pos + 5 > runs.len() {
        return Err(Some(RejectReason::Truncated {
            stage: Stage::MiddleGuard,
        }));
    }
    let mid = &runs[pos..pos + 5];
    if mid[0].dark
        || !mid[1].dark
        || mid[2].dark
        || !mid[3].dark
        || mid[4].dark
        || !guard_match(
            &[mid[0].len, mid[1].len, mid[2].len, mid[3].len, mid[4].len],
            m,
        )
    {
        return Err(Some(RejectReason::GuardMismatch {
            stage: Stage::MiddleGuard,
        }));
    }
    pos += 5;

    // --- six right digits (R parity: runs lead with dark) ---
    for d in 0..6 {
        if pos + 4 > runs.len() {
            return Err(Some(RejectReason::Truncated {
                stage: Stage::RightDigits,
            }));
        }
        let group = &runs[pos..pos + 4];
        if !group[0].dark {
            return Err(Some(RejectReason::StructureMismatch {
                stage: Stage::RightDigits,
            }));
        }
        match match_digit(group, m, Parity::LeftOnly) {
            Some((digit, _is_g, _err)) => digits[7 + d] = digit,
            None => {
                return Err(Some(RejectReason::DigitUndecodable {
                    position: 7 + d,
                }))
            }
        }
        pos += 4;
    }

    // --- end guard: [dark, light, dark] ---
    if pos + 3 > runs.len() {
        return Err(Some(RejectReason::Truncated {
            stage: Stage::EndGuard,
        }));
    }
    let eg = &runs[pos..pos + 3];
    if !(eg[0].dark && !eg[1].dark && eg[2].dark)
        || !guard_match(&[eg[0].len, eg[1].len, eg[2].len], m)
    {
        return Err(Some(RejectReason::GuardMismatch {
            stage: Stage::EndGuard,
        }));
    }

    // --- right quiet zone ---
    let quiet_right = if pos + 4 < runs.len() {
        runs[pos + 3].len
    } else {
        // Light run from end of guard to scanline edge.
        let edge = runs.last().map_or(0, |r| r.start + r.len);
        edge.saturating_sub(eg[2].start + eg[2].len)
    };
    if (quiet_right as f32) < (QUIET_RIGHT_MODULES - 1.0) * m {
        return Err(Some(RejectReason::QuietZone {
            side: QuietSide::Right,
            found_modules: quiet_right as f32 / m,
            required_modules: QUIET_RIGHT_MODULES,
        }));
    }

    // --- mod-10 checksum (ISO/IEC 15420) ---
    let sum: u32 = digits
        .iter()
        .enumerate()
        .map(|(idx, &d)| {
            let w = if idx % 2 == 0 { 1u32 } else { 3u32 };
            w * u32::from(d)
        })
        .sum();
    if sum % 10 != 0 {
        let payload: u32 = digits[..12]
            .iter()
            .enumerate()
            .map(|(idx, &d)| {
                let w = if idx % 2 == 0 { 1u32 } else { 3u32 };
                w * u32::from(d)
            })
            .sum();
        let expected = ((10 - payload % 10) % 10) as u8;
        return Err(Some(RejectReason::ChecksumMismatch {
            expected,
            actual: digits[12],
        }));
    }

    Ok(RowHit {
        x0: g[0].start,
        x1: eg[2].start + eg[2].len,
        digits,
    })
}

/// All guard runs within `GUARD_TOLERANCE * m` of one module.
fn guard_match(lens: &[u32], m: f32) -> bool {
    lens.iter()
        .all(|&l| (l as f32 - m).abs() <= GUARD_TOLERANCE * m)
}

/// Which parities `match_digit` may consider.
#[derive(Debug, Clone, Copy)]
enum Parity {
    /// Left group: compare both L and G (reversed) vectors.
    Both,
    /// Right group: R parity shares the L run-length vector.
    LeftOnly,
}

/// Match a four-run digit group against the spec patterns.
///
/// Returns `(digit, used_g_parity, summed_error)` for the best candidate
/// within tolerance, or `None` when no pattern is close enough.
fn match_digit(group: &[Run], m: f32, parity: Parity) -> Option<(u8, bool, f32)> {
    let scale = group.iter().map(|r| r.len).sum::<u32>() as f32 / 7.0;
    // Scale from the digit's own 7 modules absorbs uniform stretch; the
    // estimate `m` is only used for the tolerance bound.
    let _ = m;
    let mut best: Option<(u8, bool, f32)> = None;
    for (d, pat) in L_RUNS.iter().enumerate() {
        let err_l = run_error(group, pat, scale);
        consider(&mut best, d as u8, false, err_l, scale);
        if matches!(parity, Parity::Both) {
            let rev = [pat[3], pat[2], pat[1], pat[0]];
            let err_g = run_error(group, &rev, scale);
            consider(&mut best, d as u8, true, err_g, scale);
        }
    }
    best.filter(|&(_, _, err)| err <= DIGIT_TOLERANCE * scale)
}

fn consider(best: &mut Option<(u8, bool, f32)>, d: u8, g: bool, err: f32, _scale: f32) {
    if best.is_none_or(|&(_, _, e)| err < e) {
        *best = Some((d, g, err));
    }
}

/// Summed absolute deviation of run lengths from `pattern * scale`.
fn run_error(group: &[Run], pattern: &[u8; 4], scale: f32) -> f32 {
    group
        .iter()
        .zip(pattern)
        .map(|(r, &p)| (r.len as f32 - f32::from(p) * scale).abs())
        .sum()
}

/// Merge row hits into groups keyed by digit content; each group becomes
/// one logical symbol with a vertical extent.
pub(crate) struct HitGroup {
    pub(crate) digits: [u8; 13],
    pub(crate) x0: u32,
    pub(crate) x1: u32,
    /// Row indices (or column indices for vertical scans) contributing.
    pub(crate) rows: Vec<u32>,
}

/// Group hits from multiple scanlines by identical digit content, only
/// merging hits whose horizontal extents substantially overlap.
pub(crate) fn group_hits(hits: &[(u32, RowHit)]) -> Vec<HitGroup> {
    let mut groups: Vec<HitGroup> = Vec::new();
    for &(line, hit) in hits {
        let overlapping = groups.iter_mut().find(|g| {
            g.digits == hit.digits && ranges_overlap(g.x0, g.x1, hit.x0, hit.x1)
        });
        match overlapping {
            Some(g) => {
                g.x0 = g.x0.min(hit.x0);
                g.x1 = g.x1.max(hit.x1);
                g.rows.push(line);
            }
            None => groups.push(HitGroup {
                digits: hit.digits,
                x0: hit.x0,
                x1: hit.x1,
                rows: vec![line],
            }),
        }
    }
    groups
}

/// Two symbol extents overlap when they share at least 60% of the
/// narrower one.
fn ranges_overlap(a0: u32, a1: u32, b0: u32, b1: u32) -> bool {
    let shared = a1.min(b1).saturating_sub(a0.max(b0));
    let narrow = (a1 - a0).min(b1 - b0);
    narrow == 0 || shared as f64 >= 0.6 * f64::from(narrow)
}

/// Mod-10 check digit for the first 12 digits of an EAN-13 payload.
/// Exposed for tests and fixture renderers.
#[cfg(test)]
pub(crate) fn check_digit(first12: &[u8; 12]) -> u8 {
    let sum: u32 = first12
        .iter()
        .enumerate()
        .map(|(i, &d)| u32::from(d) * if i % 2 == 0 { 1 } else { 3 })
        .sum();
    ((10 - sum % 10) % 10) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_known_vectors() {
        // 5901234123457 is a widely published valid EAN-13.
        assert_eq!(check_digit(&[5, 9, 0, 1, 2, 3, 4, 1, 2, 3, 4, 5]), 7);
        // UPC-A 036000291452.
        assert_eq!(check_digit(&[0, 3, 6, 0, 0, 0, 2, 9, 1, 4, 5, 2]), 2);
    }

    #[test]
    fn digit_matcher_recovers_patterns() {
        // Encode digit 4 L-parity at scale 2: runs [1,1,3,2]*2 -> [2,2,6,4].
        let group = [
            Run {
                dark: false,
                start: 0,
                len: 2,
            },
            Run {
                dark: true,
                start: 2,
                len: 2,
            },
            Run {
                dark: false,
                start: 4,
                len: 6,
            },
            Run {
                dark: true,
                start: 10,
                len: 4,
            },
        ];
        let (d, g, err) = match_digit(&group, 2.0, Parity::Both).unwrap();
        assert_eq!((d, g), (4, false));
        assert!(err < 0.01);
    }
}
