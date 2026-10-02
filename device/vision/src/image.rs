//! Internal raster types: luminance extraction, adaptive binarization and
//! a packed bit image with run-length row access.

#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

use crate::{CpuFrame, FrameFormat};

/// A single-channel 8-bit luminance image.
#[derive(Debug, Clone)]
pub struct GrayImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl GrayImage {
    /// Extract luminance from a CPU frame.
    ///
    /// Packed RGB formats are converted with integer BT.601 weights;
    /// planar YUV formats reuse the luma plane directly (subsampled chroma
    /// is ignored).
    pub fn from_cpu_frame(frame: &CpuFrame<'_>) -> Self {
        let (w, h) = (frame.width() as usize, frame.height() as usize);
        let plane = frame.plane0();
        let mut data = vec![0u8; w * h];
        match frame.format() {
            FrameFormat::Luma8 | FrameFormat::Nv12 | FrameFormat::I420 => {
                for (y, row) in data.chunks_exact_mut(w).enumerate() {
                    let src = &plane.data[y * plane.stride..y * plane.stride + w];
                    row.copy_from_slice(src);
                }
            }
            FrameFormat::Rgb8 | FrameFormat::Bgr8 | FrameFormat::Rgba8 | FrameFormat::Bgra8 => {
                let bpp = frame.format().plane0_bytes_per_pixel();
                let red_first = frame.format().is_red_first();
                let (ri, bi) = if red_first {
                    (0usize, 2usize)
                } else {
                    (2usize, 0usize)
                };
                for (y, row) in data.chunks_exact_mut(w).enumerate() {
                    let src = &plane.data[y * plane.stride..y * plane.stride + w * bpp];
                    for (x, px) in row.iter_mut().enumerate() {
                        let p = &src[x * bpp..x * bpp + bpp];
                        // BT.601: 0.299 R + 0.587 G + 0.114 B in fixed point.
                        let luma =
                            u32::from(p[ri]) * 77 + u32::from(p[1]) * 150 + u32::from(p[bi]) * 29;
                        *px = (luma >> 8) as u8;
                    }
                }
            }
        }
        Self {
            width: frame.width(),
            height: frame.height(),
            data,
        }
    }
}

/// A packed binary image (`true` = dark module / bar).
#[derive(Debug, Clone)]
pub struct BitImage {
    pub width: u32,
    pub height: u32,
    row_words: usize,
    words: Vec<u64>,
}

impl BitImage {
    pub fn new(width: u32, height: u32) -> Self {
        let row_words = (width as usize).div_ceil(64);
        Self {
            width,
            height,
            row_words,
            words: vec![0u64; row_words * height as usize],
        }
    }

    /// Bit at `(x, y)`; `false` out of bounds.
    pub fn get(&self, x: u32, y: u32) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        let word = self.words[y as usize * self.row_words + x as usize / 64];
        (word >> (63 - x % 64)) & 1 == 1
    }

    pub fn set(&mut self, x: u32, y: u32, v: bool) {
        let idx = y as usize * self.row_words + x as usize / 64;
        let mask = 1u64 << (63 - x % 64);
        if v {
            self.words[idx] |= mask;
        } else {
            self.words[idx] &= !mask;
        }
    }

    /// Run-length profile of row `y`: one [`Run`] per maximal same-value
    /// span, in column order, starting with whatever the first pixel is.
    pub fn row_runs(&self, y: u32) -> Vec<Run> {
        let mut runs = Vec::new();
        if y >= self.height {
            return runs;
        }
        let mut cur = self.get(0, y);
        let mut start = 0u32;
        for x in 1..self.width {
            let b = self.get(x, y);
            if b != cur {
                runs.push(Run {
                    dark: cur,
                    start,
                    len: x - start,
                });
                cur = b;
                start = x;
            }
        }
        runs.push(Run {
            dark: cur,
            start,
            len: self.width - start,
        });
        runs
    }

    /// Run-length profile of column `x`, top to bottom.
    pub fn column_runs(&self, x: u32) -> Vec<Run> {
        let mut runs = Vec::new();
        if x >= self.width {
            return runs;
        }
        let mut cur = self.get(x, 0);
        let mut start = 0u32;
        for y in 1..self.height {
            let b = self.get(x, y);
            if b != cur {
                runs.push(Run {
                    dark: cur,
                    start,
                    len: y - start,
                });
                cur = b;
                start = y;
            }
        }
        runs.push(Run {
            dark: cur,
            start,
            len: self.height - start,
        });
        runs
    }
}

/// One maximal same-value run along a scanline.
#[derive(Debug, Clone, Copy)]
pub struct Run {
    /// `true` when the run is dark (a bar).
    pub dark: bool,
    /// Pixel offset of the run's first pixel along the axis.
    pub start: u32,
    /// Run length in pixels.
    pub len: u32,
}

/// Adaptive local-mean binarization.
///
/// Each pixel is compared against the mean luminance of a window of size
/// `max(8, min(width, height) / 8)` centred on it, via a summed-area table.
/// A pixel is dark when `lum < mean * (1 - bias)`; `bias` defaults to 0.15
/// here. Low-variance windows fall back to the global mean, which keeps
/// uniform backgrounds quiet.
pub fn binarize(gray: &GrayImage) -> BitImage {
    binarize_with_bias(gray, 0.15)
}

/// Binarize with an explicit contrast bias.
pub fn binarize_with_bias(gray: &GrayImage, bias: f64) -> BitImage {
    let (w, h) = (gray.width as usize, gray.height as usize);
    // Summed-area table in u64: sat[y][x] = sum of pixels with x' < x, y' < y.
    let mut sat = vec![0u64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row_sum = 0u64;
        for x in 0..w {
            row_sum += u64::from(gray.data[y * w + x]);
            sat[(y + 1) * (w + 1) + x + 1] = sat[y * (w + 1) + x + 1] + row_sum;
        }
    }
    let global_mean = sat[h * (w + 1) + w] as f64 / (w * h).max(1) as f64;
    let window = (gray.width.min(gray.height) / 8).clamp(8, 64);
    let half = window / 2;
    let mut out = BitImage::new(gray.width, gray.height);
    for y in 0..h {
        let y0 = y.saturating_sub(half as usize);
        let y1 = (y + half as usize + 1).min(h);
        for x in 0..w {
            let x0 = x.saturating_sub(half as usize);
            let x1 = (x + half as usize + 1).min(w);
            let area = ((y1 - y0) * (x1 - x0)) as f64;
            let sum = sat[y1 * (w + 1) + x1] + sat[y0 * (w + 1) + x0]
                - sat[y0 * (w + 1) + x1]
                - sat[y1 * (w + 1) + x0];
            let mean = sum as f64 / area;
            // Low local contrast -> use global mean instead of the noisy local one.
            let threshold = if (mean - global_mean).abs() < 8.0 {
                global_mean
            } else {
                mean
            };
            // A uniformly dark window pulls the mean down to the pixel
            // value itself; floor the threshold at the global mean so a
            // bar interior stays dark.
            let dark = f64::from(gray.data[y * w + x]) < threshold.max(global_mean) * (1.0 - bias);
            out.set(x as u32, y as u32, dark);
        }
    }
    out
}

/// Inverted copy of a bit image (dark <-> light).
pub fn invert(src: &BitImage) -> BitImage {
    let mut out = src.clone();
    for w in &mut out.words {
        *w = !*w;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binarize_contrasting_bars() {
        let mut g = GrayImage {
            width: 8,
            height: 8,
            data: vec![200; 64],
        };
        for y in 0..8 {
            g.data[y * 8 + 3] = 20;
            g.data[y * 8 + 4] = 20;
        }
        let bits = binarize(&g);
        for y in 0..8 {
            assert!(!bits.get(0, y));
            assert!(bits.get(3, y));
            assert!(bits.get(4, y));
            assert!(!bits.get(7, y));
        }
    }

    #[test]
    fn row_runs_matches_set_bits() {
        let mut img = BitImage::new(10, 3);
        img.set(2, 1, true);
        img.set(3, 1, true);
        img.set(7, 1, true);
        let runs = img.row_runs(1);
        // light[0..2] dark[2..4] light[4..7] dark[7..8] light[8..10]
        assert_eq!(runs.len(), 5);
        assert_eq!((runs[1].dark, runs[1].start, runs[1].len), (true, 2, 2));
        assert_eq!((runs[3].dark, runs[3].start, runs[3].len), (true, 7, 1));
    }
}
