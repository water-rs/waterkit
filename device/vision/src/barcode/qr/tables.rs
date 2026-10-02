//! QR Code Model 2 constant tables and BCH decoding, per ISO/IEC 18004.
//!
//! Covers versions 1..=10 only — the supported slice.

#![allow(clippy::cast_possible_truncation, clippy::missing_const_for_fn)]

/// Error-correction level. Bit values are the format-information encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EcLevel {
    /// L — approx. 7 % recovery.
    L,
    /// M — approx. 15 % recovery.
    M,
    /// Q — approx. 25 % recovery.
    Q,
    /// H — approx. 30 % recovery.
    H,
}

impl EcLevel {
    /// Level for the two EC bits of decoded format information.
    /// Encoding: `00`=M, `01`=L, `10`=H, `11`=Q.
    pub fn from_bits(bits: u8) -> Self {
        match bits & 0x03 {
            0 => Self::M,
            1 => Self::L,
            2 => Self::H,
            _ => Self::Q,
        }
    }
}

/// Alignment-pattern centre coordinates per version (ISO/IEC 18004
/// table E.1). Versions 2..=6 use the single `dim - 7` centre; 7..=10
/// add intermediate centres.
pub fn alignment_centers(version: u8) -> &'static [u8] {
    match version {
        2 => &[6, 18],
        3 => &[6, 22],
        4 => &[6, 26],
        5 => &[6, 30],
        6 => &[6, 34],
        7 => &[6, 22, 38],
        8 => &[6, 24, 42],
        9 => &[6, 26, 46],
        10 => &[6, 28, 50],
        _ => &[],
    }
}

/// One group of identically-shaped error-correction blocks.
#[derive(Debug, Clone, Copy)]
pub struct BlockGroup {
    /// Number of blocks in the group.
    pub count: usize,
    /// Data codewords per block.
    pub data_len: usize,
}

/// EC structure for a `(version, level)` pair: `ec_len` check symbols
/// per block, then the block groups.
#[derive(Debug, Clone, Copy)]
pub struct EcStructure {
    /// Check symbols appended to every block.
    pub ec_len: usize,
    /// Up to two groups; the second group's blocks carry one more data
    /// codeword than the first's.
    pub groups: [BlockGroup; 2],
}

impl EcStructure {
    /// Total codewords (data + EC) stored in the symbol.
    pub fn total(&self) -> usize {
        self.groups
            .iter()
            .map(|g| g.count * (g.data_len + self.ec_len))
            .sum()
    }

    /// Data-codeword count summed over all blocks.
    pub fn data_len(&self) -> usize {
        self.groups.iter().map(|g| g.count * g.data_len).sum()
    }
}

const fn b(count: usize, data_len: usize) -> BlockGroup {
    BlockGroup { count, data_len }
}

/// EC block structure per ISO/IEC 18004 table 9 (versions 1..=10).
///
/// Returns `None` for anything outside the supported range.
pub fn ec_structure(version: u8, level: EcLevel) -> Option<EcStructure> {
    use EcLevel::{H, L, M, Q};
    let (ec_len, groups) = match (version, level) {
        (1, L) => (7, [b(1, 19), b(0, 0)]),
        (1, M) => (10, [b(1, 16), b(0, 0)]),
        (1, Q) => (13, [b(1, 13), b(0, 0)]),
        (1, H) => (17, [b(1, 9), b(0, 0)]),
        (2, L) => (10, [b(1, 34), b(0, 0)]),
        (2, M) => (16, [b(1, 28), b(0, 0)]),
        (2, Q) => (22, [b(1, 22), b(0, 0)]),
        (2, H) => (28, [b(1, 16), b(0, 0)]),
        (3, L) => (15, [b(1, 55), b(0, 0)]),
        (3, M) => (26, [b(1, 44), b(0, 0)]),
        (3, Q) => (18, [b(2, 17), b(0, 0)]),
        (3, H) => (22, [b(2, 13), b(0, 0)]),
        (4, L) => (20, [b(1, 80), b(0, 0)]),
        (4, M) => (18, [b(2, 32), b(0, 0)]),
        (4, Q) => (26, [b(2, 24), b(0, 0)]),
        (4, H) => (16, [b(4, 9), b(0, 0)]),
        (5, L) => (26, [b(1, 108), b(0, 0)]),
        (5, M) => (24, [b(2, 43), b(0, 0)]),
        (5, Q) => (18, [b(2, 15), b(2, 16)]),
        (5, H) => (22, [b(2, 11), b(2, 12)]),
        (6, L) => (18, [b(2, 68), b(0, 0)]),
        (6, M) => (16, [b(4, 27), b(0, 0)]),
        (6, Q) => (24, [b(4, 19), b(0, 0)]),
        (6, H) => (28, [b(4, 15), b(0, 0)]),
        (7, L) => (20, [b(2, 78), b(0, 0)]),
        (7, M) => (18, [b(4, 31), b(0, 0)]),
        (7, Q) => (18, [b(2, 14), b(4, 15)]),
        (7, H) => (26, [b(4, 13), b(1, 14)]),
        (8, L) => (24, [b(2, 97), b(0, 0)]),
        (8, M) => (22, [b(2, 38), b(2, 39)]),
        (8, Q) => (22, [b(4, 18), b(2, 19)]),
        (8, H) => (26, [b(4, 14), b(2, 15)]),
        (9, L) => (30, [b(2, 116), b(0, 0)]),
        (9, M) => (22, [b(3, 36), b(2, 37)]),
        (9, Q) => (20, [b(4, 16), b(4, 17)]),
        (9, H) => (24, [b(4, 12), b(4, 13)]),
        (10, L) => (18, [b(2, 68), b(2, 69)]),
        (10, M) => (26, [b(4, 43), b(1, 44)]),
        (10, Q) => (24, [b(6, 19), b(2, 20)]),
        (10, H) => (28, [b(6, 15), b(2, 16)]),
        _ => return None,
    };
    Some(EcStructure { ec_len, groups })
}

/// Polynomial remainder `v mod g` in GF(2) (binary long division).
const fn remainder(mut v: u32, g: u32) -> u32 {
    let gd = g.ilog2();
    loop {
        if v == 0 {
            return 0;
        }
        let vd = v.ilog2();
        if vd < gd {
            return v;
        }
        v ^= g << (vd - gd);
    }
}

/// Format-information generator BCH(15,5), `x¹⁰+x⁸+x⁵+x⁴+x²+x+1`.
const FORMAT_GEN: u32 = 0b101_0011_0111;
/// Format-information XOR mask (ISO/IEC 18004 8.9 / table 24 note).
const FORMAT_MASK: u32 = 0b101_0100_0001_0010;

/// Masked format-information codeword for the five data bits `data`
/// (`ec_level << 3 | mask_pattern`).
const fn format_word(data: u32) -> u32 {
    let v = data << 10;
    (v | remainder(v, FORMAT_GEN)) ^ FORMAT_MASK
}

const fn format_words() -> [u32; 32] {
    let mut t = [0u32; 32];
    let mut i = 0usize;
    while i < 32 {
        t[i] = format_word(i as u32);
        i += 1;
    }
    t
}

/// All 32 legal (masked) format-information words.
const FORMAT_WORDS: [u32; 32] = format_words();

/// Version-information generator BCH(18,6), `x¹²+x¹¹+x¹⁰+x⁹+x⁸+x⁵+x²+1`.
const VERSION_GEN: u32 = 0b1_1111_0010_0101;

/// Version-information codeword for version `v` (18 bits, no mask).
const fn version_word(v: u32) -> u32 {
    let s = v << 12;
    s | remainder(s, VERSION_GEN)
}

const fn version_words() -> [u32; 34] {
    let mut t = [0u32; 34];
    let mut i = 0usize;
    while i < 34 {
        t[i] = version_word(i as u32 + 7);
        i += 1;
    }
    t
}

/// Version-information codewords for versions 7..=40, indexed `v - 7`.
const VERSION_WORDS: [u32; 34] = version_words();

/// Hamming distance between two 32-bit words.
fn distance(a: u32, b: u32) -> u8 {
    (a ^ b).count_ones() as u8
}

/// Decode `received` against `codebook`, tolerating up to `max` flipped
/// bits. Returns `(data_bits, distance)` of the closest word.
fn bch_decode(received: u32, codebook: &[u32], max: u8) -> Option<(u32, u8)> {
    let mut best: Option<(u32, u8)> = None;
    for (i, &word) in codebook.iter().enumerate() {
        let d = distance(received, word);
        if best.is_none_or(|(_, bd)| d < bd) {
            best = Some((i as u32, d));
        }
    }
    best.filter(|&(_, d)| d <= max)
}

/// Decode a 15-bit format-information word. Returns the five data bits
/// (`ec_bits << 3 | mask`) and the corrected distance, or `None` when
/// every legal word is more than 3 bits away.
pub fn decode_format(received: u32) -> Option<(u32, u8)> {
    bch_decode(received, &FORMAT_WORDS, 3)
}

/// Decode an 18-bit version-information word. Returns the version and
/// the corrected distance, or `None` when no word is within 3 bits.
pub fn decode_version(received: u32) -> Option<(u8, u8)> {
    bch_decode(received, &VERSION_WORDS, 3).map(|(i, d)| (i as u8 + 7, d))
}

/// Data-bit mask function `f(row, col)` for mask pattern `mask`
/// (ISO/IEC 18004 table 20).
pub fn mask_bit(mask: u8, row: u32, col: u32) -> bool {
    match mask {
        0 => (row + col).is_multiple_of(2),
        1 => row.is_multiple_of(2),
        2 => col.is_multiple_of(3),
        3 => (row + col).is_multiple_of(3),
        4 => (row / 2 + col / 3).is_multiple_of(2),
        5 => (row * col) % 2 + (row * col) % 3 == 0,
        6 => ((row * col) % 2 + (row * col) % 3).is_multiple_of(2),
        7 => ((row + col) % 2 + (row * col) % 3).is_multiple_of(2),
        _ => false,
    }
}

/// Alphanumeric character set (ISO/IEC 18004 table 5), index = code.
pub const ALPHANUMERIC: &[u8; 45] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";

/// Character-count field width per mode and version group
/// (v1–9, v10–26, v27–40 — only the first two are reachable here).
pub fn char_count_bits(mode: u8, version: u8) -> u8 {
    let group = u8::from(version > 9);
    match (mode, group) {
        (1, 0) => 10,
        (1, _) => 12,
        (2, 0) => 9,
        (2, _) => 11,
        (4, 0) => 8,
        (4, _) => 16,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_info_known_words() {
        // ISO/IEC 18004 examples: L/mask0 -> 111011111000100,
        // H/mask2 -> 001110011100111.
        assert_eq!(format_word(0b01000), 0b111_0111_1100_0100);
        assert_eq!(format_word(0b10010), 0b001_1100_1110_0111);
        let (data, dist) = decode_format(0b111_0111_1100_0100).unwrap();
        assert_eq!((data, dist), (0b01000, 0));
        // Two flipped bits still correct.
        let (data, dist) = decode_format(0b111_0111_1100_0100 ^ 0b101).unwrap();
        assert_eq!((data, dist), (0b01000, 2));
    }

    #[test]
    fn version_info_known_word() {
        // Version 7 -> 000111110010010100 (spec example).
        assert_eq!(version_word(7), 0b00_0111_1100_1001_0100);
        let (v, dist) = decode_version(0b00_0111_1100_1001_0100).unwrap();
        assert_eq!((v, dist), (7, 0));
    }

    #[test]
    fn block_table_totals() {
        // Total codewords per version, ISO/IEC 18004 table 9 last column.
        let totals = [26, 44, 70, 100, 134, 172, 196, 242, 292, 346];
        for (v, &total) in totals.iter().enumerate() {
            for level in [EcLevel::L, EcLevel::M, EcLevel::Q, EcLevel::H] {
                let s = ec_structure(v as u8 + 1, level).unwrap();
                assert_eq!(s.total(), total, "v{} {:?}", v + 1, level);
            }
        }
    }
}
