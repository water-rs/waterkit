//! Image-side QR geometry: finder-pattern detection, trio selection,
//! alignment-pattern search, perspective sampling, function-pattern
//! masking and quiet-zone probing.
//!
//! Detection scans the bit image for the finder's 1:1:3:1:1 run-length
//! signature on horizontal (or vertical, per [`ScanAxes`]) scanlines,
//! cross-checks each candidate vertically and diagonally, and clusters
//! repeated detections into candidate centres. Triples of centres are
//! scored by finder geometry (two equal legs plus a diagonal at √2);
//! the best triples are decoded in order.

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::suboptimal_flops,
    clippy::too_many_lines,
    clippy::many_single_char_names,
    clippy::option_if_let_else,
    clippy::float_cmp,
    clippy::missing_const_for_fn,
    clippy::use_self
)]

use crate::ScanAxes;
use crate::barcode::QuietSide;
use crate::geometry::{Homography, Point};
use crate::image::{BitImage, Run};

/// A merged finder-pattern centre: estimated centre in pixel
/// coordinates, estimated module size, and the number of scanline
/// detections that merged into it.
#[derive(Debug, Clone, Copy)]
pub struct Center {
    /// Pixel x of the finder centre.
    pub x: f64,
    /// Pixel y of the finder centre.
    pub y: f64,
    /// Estimated module width in pixels.
    pub module: f64,
    /// Supporting scanline detections.
    pub count: u32,
}

impl Center {
    fn point(&self) -> Point {
        Point::new(self.x, self.y)
    }

    fn merge(&mut self, other: &Center) {
        let total = self.count + other.count;
        let w = f64::from(other.count) / f64::from(total);
        self.x = self.x.mul_add(1.0 - w, other.x * w);
        self.y = self.y.mul_add(1.0 - w, other.y * w);
        self.module = self.module.mul_add(1.0 - w, other.module * w);
        self.count = total;
    }
}

/// A plausible finder-pattern triple, oriented into module space:
/// `tl` is the top-left pattern (the one opposite the diagonal pair).
#[derive(Debug, Clone, Copy)]
pub struct Trio {
    /// Top-left finder centre (module coordinates).
    pub tl: Center,
    /// Top-right finder centre.
    pub tr: Center,
    /// Bottom-left finder centre.
    pub bl: Center,
    /// Mean estimated module size in pixels.
    pub module: f64,
    /// Geometric plausibility (lower is better).
    pub score: f64,
}

/// Collect finder candidates from a binarized image.
///
/// Horizontal axes scan rows and cross-check columns; vertical axes do
/// the opposite — a rotated code still resolves because the finder
/// signature is isotropic.
pub fn find_centers(bits: &BitImage, axes: ScanAxes) -> (Vec<Center>, u32) {
    let mut centers: Vec<Center> = Vec::new();
    let horizontal = matches!(axes, ScanAxes::Horizontal | ScanAxes::Both);
    let vertical = matches!(axes, ScanAxes::Vertical | ScanAxes::Both);
    let lines = if horizontal { bits.height } else { bits.width };
    if horizontal {
        for y in 0..bits.height {
            scan_runs(&bits.row_runs(y), |cx| {
                if let Some(c) = cross_check(bits, cx, y, true) {
                    push_center(&mut centers, c);
                }
            });
        }
    }
    if vertical {
        for x in 0..bits.width {
            scan_runs(&bits.column_runs(x), |cy| {
                if let Some(c) = cross_check(bits, cy, x, false) {
                    push_center(&mut centers, c);
                }
            });
        }
    }
    (centers, lines)
}

/// Scan one run list for the finder signature 1:1:3:1:1
/// (dark-light-dark-light-dark) and report each candidate centre
/// position along the scan axis.
fn scan_runs(runs: &[Run], mut hit: impl FnMut(f64)) {
    let mut i = 0usize;
    while i + 5 <= runs.len() {
        let g = &runs[i..i + 5];
        if g[0].dark && !g[1].dark && g[2].dark && !g[3].dark && g[4].dark {
            let total = g.iter().map(|r| r.len).sum::<u32>();
            if total >= 7 {
                let m = total as f64 / 7.0;
                let ok = |len: u32, expect: f64| (len as f64 - expect).abs() < expect * 0.5 + 0.5;
                if ok(g[0].len, m)
                    && ok(g[1].len, m)
                    && ok(g[2].len, 3.0 * m)
                    && ok(g[3].len, m)
                    && ok(g[4].len, m)
                {
                    let end = g[4].start + g[4].len;
                    hit(end as f64 - g[4].len as f64 - g[3].len as f64 - g[2].len as f64 / 2.0);
                }
            }
        }
        i += 1;
    }
}

/// Count the 5-run profile outward both ways from a seed pixel along a
/// line, returning the five run lengths `[outer_dark, light, centre_dark,
/// light, outer_dark]` and the centre run's midpoint.
#[allow(clippy::too_many_arguments)]
fn cross_profile(bits: &BitImage, cx: i32, cy: i32, dx: i32, dy: i32) -> Option<(Point, f64)> {
    if cx < 0 || cy < 0 || cx >= bits.width as i32 || cy >= bits.height as i32 {
        return None;
    }
    if !bits.get(cx as u32, cy as u32) {
        return None;
    }
    let inside =
        |x: i32, y: i32| x >= 0 && y >= 0 && x < bits.width as i32 && y < bits.height as i32;
    let mut counts = [0u32; 5];
    // Negative direction: centre dark (incl. seed), light, outer dark.
    let (mut x, mut y) = (cx, cy);
    while inside(x, y) && bits.get(x as u32, y as u32) {
        counts[2] += 1;
        x -= dx;
        y -= dy;
    }
    let up_centre = counts[2];
    while inside(x, y) && !bits.get(x as u32, y as u32) {
        counts[1] += 1;
        x -= dx;
        y -= dy;
    }
    while inside(x, y) && bits.get(x as u32, y as u32) {
        counts[0] += 1;
        x -= dx;
        y -= dy;
    }
    // Positive direction: rest of centre dark, light, outer dark.
    let (mut x, mut y) = (cx + dx, cy + dy);
    while inside(x, y) && bits.get(x as u32, y as u32) {
        counts[2] += 1;
        x += dx;
        y += dy;
    }
    while inside(x, y) && !bits.get(x as u32, y as u32) {
        counts[3] += 1;
        x += dx;
        y += dy;
    }
    while inside(x, y) && bits.get(x as u32, y as u32) {
        counts[4] += 1;
        x += dx;
        y += dy;
    }
    if counts.contains(&0) {
        return None;
    }
    let total: u32 = counts.iter().sum();
    if total < 7 {
        return None;
    }
    let m = f64::from(total) / 7.0;
    let ok = |len: u32, expect: f64| (f64::from(len) - expect).abs() < expect * 0.55 + 0.6;
    if !(ok(counts[0], m)
        && ok(counts[1], m)
        && ok(counts[2], 3.0 * m)
        && ok(counts[3], m)
        && ok(counts[4], m))
    {
        return None;
    }
    // The centre run spans `cx - (up_centre - 1)` .. `cx + (counts[2] -
    // up_centre)` along `d`; its midpoint is the refined centre.
    let off = (f64::from(counts[2]) + 1.0 - 2.0 * f64::from(up_centre)) / 2.0;
    Some((
        Point::new(
            f64::from(cx) + f64::from(dx) * off,
            f64::from(cy) + f64::from(dy) * off,
        ),
        m,
    ))
}

/// Confirm a scanline hit by cross-checking through the centre:
/// perpendicular direction first, then both diagonals. Returns the
/// refined centre on success.
///
/// `hit_pos` is the detected centre along the scanned axis; `line_idx`
/// is the row (horizontal scans) or column (vertical scans) index.
fn cross_check(bits: &BitImage, hit_pos: f64, line_idx: u32, horizontal: bool) -> Option<Center> {
    let (cx, cy) = if horizontal {
        (hit_pos.round() as i32, line_idx as i32)
    } else {
        (line_idx as i32, hit_pos.round() as i32)
    };
    // Perpendicular profile.
    let (p_perp, m_perp) = if horizontal {
        cross_profile(bits, cx, cy, 0, 1)?
    } else {
        cross_profile(bits, cx, cy, 1, 0)?
    };
    // Diagonal profiles through the refined centre.
    let ix = p_perp.x.round() as i32;
    let iy = p_perp.y.round() as i32;
    let d1 = cross_profile(bits, ix, iy, 1, 1)?;
    let d2 = cross_profile(bits, ix, iy, 1, -1)?;
    // Consensus: perpendicular and diagonal centres must agree.
    let center = Point::new(
        (p_perp.x + d1.0.x + d2.0.x) / 3.0,
        (p_perp.y + d1.0.y + d2.0.y) / 3.0,
    );
    let m = (m_perp + d1.1 + d2.1) / 3.0;
    Some(Center {
        x: center.x,
        y: center.y,
        module: m,
        count: 1,
    })
}

/// Merge a fresh detection into the candidate list, or append it.
fn push_center(centers: &mut Vec<Center>, new: Center) {
    if let Some(c) = centers.iter_mut().find(|c| {
        (c.x - new.x).abs() < c.module.max(new.module)
            && (c.y - new.y).abs() < c.module.max(new.module)
    }) {
        c.merge(&new);
    } else {
        centers.push(new);
    }
}

/// Distance between two points.
fn dist(a: Point, b: Point) -> f64 {
    a.distance(b)
}

/// Build the plausible finder triples, best first.
///
/// A true triple has two legs of ~equal length meeting at the
/// top-left pattern and a √2 diagonal between the other two.
pub fn trios(centers: &[Center]) -> Vec<Trio> {
    let mut out: Vec<Trio> = Vec::new();
    let n = centers.len().min(12);
    for i in 0..n {
        for j in (i + 1)..n {
            for k in (j + 1)..n {
                if let Some(t) = make_trio(centers[i], centers[j], centers[k]) {
                    out.push(t);
                }
            }
        }
    }
    out.sort_by(|a, b| a.score.total_cmp(&b.score));
    out
}

/// Order three centres into `(tl, tr, bl)` and score the geometry.
/// Returns `None` when the geometry is not finder-plausible.
fn make_trio(a: Center, b: Center, c: Center) -> Option<Trio> {
    // Module sizes must roughly agree.
    let m_min = a.module.min(b.module).min(c.module);
    let m_max = a.module.max(b.module).max(c.module);
    if m_max > m_min * 1.6 {
        return None;
    }
    let pts = [a, b, c];
    // The diagonal pair has the longest mutual distance; the leftover
    // point is the top-left pattern.
    let d = [
        dist(pts[0].point(), pts[1].point()),
        dist(pts[0].point(), pts[2].point()),
        dist(pts[1].point(), pts[2].point()),
    ];
    let (diag, tl_idx) = if d[0] >= d[1] && d[0] >= d[2] {
        ((0, 1), 2)
    } else if d[1] >= d[2] {
        ((0, 2), 1)
    } else {
        ((1, 2), 0)
    };
    let tl = pts[tl_idx];
    let (p, q) = (pts[diag.0], pts[diag.1]);
    // Cross(p - tl, q - tl) > 0 (y-down image space) => p is TR.
    let cross = (p.x - tl.x) * (q.y - tl.y) - (p.y - tl.y) * (q.x - tl.x);
    let (tr, bl) = if cross > 0.0 { (p, q) } else { (q, p) };
    // Leg lengths and diagonal must be consistent.
    let leg1 = dist(tl.point(), tr.point());
    let leg2 = dist(tl.point(), bl.point());
    let hyp = dist(tr.point(), bl.point());
    let short = leg1.min(leg2);
    if short <= 0.0 || leg1 / leg2 > 1.7 || leg2 / leg1 > 1.7 {
        return None;
    }
    let expect_hyp = short * std::f64::consts::SQRT_2;
    if (hyp - expect_hyp).abs() > expect_hyp * 0.45 {
        return None;
    }
    let module = (a.module + b.module + c.module) / 3.0;
    let leg_disp = (leg1 - leg2).abs() / short;
    let hyp_err = (hyp - expect_hyp).abs() / expect_hyp;
    let size_disp = (m_max - m_min) / m_min;
    Some(Trio {
        tl,
        tr,
        bl,
        module,
        score: leg_disp + hyp_err + size_disp,
    })
}

/// Provisional module dimension from finder geometry.
pub fn dimension_estimate(t: &Trio) -> Option<u32> {
    let leg = dist(t.tl.point(), t.tr.point()).midpoint(dist(t.tl.point(), t.bl.point()));
    let est = leg / t.module + 7.0;
    let raw = est.round() as i64;
    // Snap to the nearest `4k + 1`.
    for off in [0i64, 1, -1, 2, -2, 3, -3] {
        let cand = raw + off;
        if (cand - 21) % 4 == 0 && (21..=57).contains(&cand) {
            return Some(cand as u32);
        }
    }
    None
}

/// Locate the bottom-right alignment pattern's centre in image space.
///
/// The bottom-right alignment pattern sits at module `(dim - 7, dim - 7)`
/// for every supported version ≥ 2 (the last entry of the centre table).
/// Candidate centres in a ±1.5-module window are scored by how well the
/// 5×5 ring template matches the image sampled through `h_est` — this
/// works in module space, so adjacent data modules cannot corrupt the
/// run profile the way a scanline search would.
///
/// Returns the best-matching centre's pixel position, or `None` when no
/// candidate matches the template within tolerance.
pub fn locate_alignment(bits: &BitImage, h_est: &Homography, dim: u32) -> Option<Point> {
    let base = f64::from(dim) - 6.5;
    let mut best: Option<(u32, f64, f64, f64)> = None;
    for dy8 in -3..=3i32 {
        for dx8 in -3..=3i32 {
            let (dx, dy) = (f64::from(dx8) * 0.5, f64::from(dy8) * 0.5);
            let mut errors = 0u32;
            for dj in -2i32..=2 {
                for di in -2i32..=2 {
                    let expect_dark = di.abs() == 2 || dj.abs() == 2 || (di == 0 && dj == 0);
                    let dark = sample_point(
                        bits,
                        h_est,
                        base + dx + f64::from(di),
                        base + dy + f64::from(dj),
                    );
                    if dark != expect_dark {
                        errors += 1;
                    }
                }
            }
            let radius = dx.abs() + dy.abs();
            if best.is_none_or(|(e, _, _, r)| errors < e || (errors == e && radius < r)) {
                best = Some((errors, base + dx, base + dy, radius));
            }
        }
    }
    let (errors, mx, my, _) = best?;
    if errors > 6 {
        return None;
    }
    h_est.apply(mx, my)
}

/// Build the module-space → image transform for a symbol.
///
/// `v1` uses the three finder centres (affine). `v >= 2` anchors the
/// bottom-right corner with the alignment centre (full perspective).
pub fn transform(t: &Trio, dim: u32, alignment: Option<Point>) -> Option<Homography> {
    let tl = t.tl.point();
    let tr = t.tr.point();
    let bl = t.bl.point();
    let d = f64::from(dim);
    match alignment {
        Some(al) => Homography::from_points(
            [
                Point::new(3.5, 3.5),
                Point::new(d - 3.5, 3.5),
                Point::new(d - 6.5, d - 6.5),
                Point::new(3.5, d - 3.5),
            ],
            [tl, tr, al, bl],
        ),
        None => Homography::affine_from_points(
            [
                Point::new(3.5, 3.5),
                Point::new(d - 3.5, 3.5),
                Point::new(3.5, d - 3.5),
            ],
            [tl, tr, bl],
        ),
    }
}

/// Sample the module grid through `h` into a `dim`×`dim` matrix.
pub fn sample(bits: &BitImage, h: &Homography, dim: u32) -> Option<BitImage> {
    let mut out = BitImage::new(dim, dim);
    for y in 0..dim {
        for x in 0..dim {
            let p = h.apply(x as f64 + 0.5, y as f64 + 0.5)?;
            let (px, py) = (p.x.round() as i64, p.y.round() as i64);
            if px < 0 || py < 0 || px >= bits.width as i64 || py >= bits.height as i64 {
                return None;
            }
            if bits.get(px as u32, py as u32) {
                out.set(x, y, true);
            }
        }
    }
    Some(out)
}

/// Sample one module-space point from the source image; out-of-bounds
/// yields `false` (light).
pub fn sample_point(bits: &BitImage, h: &Homography, mx: f64, my: f64) -> bool {
    h.apply(mx, my).is_some_and(|p| {
        let (px, py) = (p.x.round() as i64, p.y.round() as i64);
        px >= 0
            && py >= 0
            && px < bits.width as i64
            && py < bits.height as i64
            && bits.get(px as u32, py as u32)
    })
}

/// The modules occupied by function patterns (never data), per
/// ISO/IEC 18004 §6.3 / Annex E.
pub fn function_mask(dim: u32, version: u8) -> BitImage {
    let mut mask = BitImage::new(dim, dim);
    let mut region = |x0: i64, y0: i64, w: i64, hgt: i64| {
        for y in y0..(y0 + hgt) {
            for x in x0..(x0 + w) {
                if x >= 0 && y >= 0 && x < dim as i64 && y < dim as i64 {
                    mask.set(x as u32, y as u32, true);
                }
            }
        }
    };
    // Finder patterns + separators + format-info regions.
    region(0, 0, 9, 9);
    region(dim as i64 - 8, 0, 8, 9);
    region(0, dim as i64 - 8, 9, 8);
    // Timing patterns (row 6 / column 6 in full).
    region(0, 6, dim as i64, 1);
    region(6, 0, 1, dim as i64);
    // Alignment patterns (skip the three corners covered by finders).
    let centers = super::tables::alignment_centers(version);
    let last = *centers.last().unwrap_or(&0);
    for &cx in centers {
        for &cy in centers {
            let at_finder = cx == 6 && (cy == 6 || cy == last) || cx == last && cy == 6;
            if !at_finder {
                region(i64::from(cx) - 2, i64::from(cy) - 2, 5, 5);
            }
        }
    }
    // Version information (versions ≥ 7).
    if version >= 7 {
        region(dim as i64 - 11, 0, 3, 6);
        region(0, dim as i64 - 11, 6, 3);
    }
    mask
}

/// Verify the timing patterns alternate correctly along row/column 6
/// between the finders. Returns the fraction of mismatched modules.
pub fn timing_error_rate(m: &BitImage, dim: u32) -> f32 {
    let mut checked = 0u32;
    let mut bad = 0u32;
    let mut check = |x: u32, y: u32, expect_dark: bool| {
        checked += 1;
        if m.get(x, y) != expect_dark {
            bad += 1;
        }
    };
    for x in 8..(dim - 8) {
        check(x, 6, x % 2 == 0);
    }
    for y in 8..(dim - 8) {
        check(6, y, y % 2 == 0);
    }
    if checked == 0 {
        0.0
    } else {
        bad as f32 / checked as f32
    }
}

/// Read the two copies of the 15-bit format information from a sampled
/// matrix (ISO/IEC 18004 figure 19 ordering).
pub fn format_bits(m: &BitImage, dim: u32) -> (u32, u32) {
    let mut copy1 = 0u32;
    for i in 0..6 {
        copy1 = (copy1 << 1) | u32::from(m.get(i, 8));
    }
    copy1 = (copy1 << 1) | u32::from(m.get(7, 8));
    copy1 = (copy1 << 1) | u32::from(m.get(8, 8));
    copy1 = (copy1 << 1) | u32::from(m.get(8, 7));
    for j in (0..6).rev() {
        copy1 = (copy1 << 1) | u32::from(m.get(8, j));
    }
    let mut copy2 = 0u32;
    for j in (dim - 7..dim).rev() {
        copy2 = (copy2 << 1) | u32::from(m.get(8, j));
    }
    for i in (dim - 8)..dim {
        copy2 = (copy2 << 1) | u32::from(m.get(i, 8));
    }
    (copy1, copy2)
}

/// Read the two 18-bit version-information copies directly from the
/// image via `h` (used before the final transform exists).
pub fn version_bits(bits: &BitImage, h: &Homography, dim: u32) -> (u32, u32) {
    let mut v1 = 0u32;
    for j in (0..6).rev() {
        for i in (dim - 11..=dim - 9).rev() {
            v1 = (v1 << 1) | u32::from(sample_point(bits, h, i as f64 + 0.5, j as f64 + 0.5));
        }
    }
    let mut v2 = 0u32;
    for i in (0..6).rev() {
        for j in (dim - 11..=dim - 9).rev() {
            v2 = (v2 << 1) | u32::from(sample_point(bits, h, i as f64 + 0.5, j as f64 + 0.5));
        }
    }
    (v1, v2)
}

/// Probe the quiet zone around the symbol through `h`.
///
/// For each side, walk module offsets 1..=4 outward; a dark module at
/// offset `k` reports `found = k - 1` modules of clear margin. Returns
/// the failing side plus observed width, or `None` when all four sides
/// are clear. Points mapped outside the image count as open (light)
/// space, matching the scanline-edge rule used for linear codes.
pub fn quiet_zone_violation(bits: &BitImage, h: &Homography, dim: u32) -> Option<(QuietSide, f32)> {
    let d = f64::from(dim);
    let probe = |mx: f64, my: f64| -> bool {
        h.apply(mx, my).is_some_and(|p| {
            let (px, py) = (p.x.round() as i64, p.y.round() as i64);
            px >= 0
                && py >= 0
                && px < bits.width as i64
                && py < bits.height as i64
                && bits.get(px as u32, py as u32)
        })
    };
    // (side, per-module point for offset k and edge coordinate i).
    let sides: [(QuietSide, u8); 4] = [
        (QuietSide::Top, 0),
        (QuietSide::Right, 1),
        (QuietSide::Bottom, 2),
        (QuietSide::Left, 3),
    ];
    for (side, axis) in sides {
        for k in 1..=4i64 {
            let mut dark_seen = false;
            for i in 0..dim as i64 {
                let t = i as f64 + 0.5;
                let off = k as f64 - 0.5;
                let (mx, my) = match axis {
                    0 => (t, -off),    // above the top edge
                    1 => (d + off, t), // right of the right edge
                    2 => (t, d + off), // below the bottom edge
                    _ => (-off, t),    // left of the left edge
                };
                if probe(mx, my) {
                    dark_seen = true;
                    break;
                }
            }
            if dark_seen {
                return Some((side, (k - 1) as f32));
            }
        }
    }
    None
}
