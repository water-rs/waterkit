//! Integration tests for the QR Code Model 2 decoder, driven by a
//! spec-faithful software encoder/renderer (no binary fixtures).

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::many_single_char_names,
    clippy::missing_const_for_fn,
    clippy::doc_markdown,
    clippy::match_same_arms,
    clippy::too_many_lines,
    clippy::needless_range_loop,
    clippy::bool_to_int_with_if,
    clippy::nonminimal_bool,
    clippy::float_cmp
)]

use waterkit_vision::{
    BarcodeEngine, CpuFrame, DecodeOptions, Formats, RejectReason, ScanAxes, Symbology,
};

// ---------------------------------------------------------------- GF(256)

const EXP: [u8; 512] = build_exp();
const LOG: [u8; 256] = build_log();

const fn build_exp() -> [u8; 512] {
    let mut e = [0u8; 512];
    let mut a: u16 = 1;
    let mut i = 0usize;
    while i < 255 {
        e[i] = a as u8;
        a <<= 1;
        if a & 0x100 != 0 {
            a ^= 0x11D;
        }
        i += 1;
    }
    while i < 512 {
        e[i] = e[i - 255];
        i += 1;
    }
    e
}

const fn build_log() -> [u8; 256] {
    let mut l = [0u8; 256];
    let mut i = 0usize;
    while i < 255 {
        l[EXP[i] as usize] = i as u8;
        i += 1;
    }
    l
}

fn gmul(a: u8, b: u8) -> u8 {
    if a == 0 || b == 0 {
        0
    } else {
        EXP[LOG[a as usize] as usize + LOG[b as usize] as usize]
    }
}

/// Generator polynomial (x - α⁰)…(x - α^{ec-1}), ascending-degree form.
fn rs_generator(ec_len: usize) -> Vec<u8> {
    let mut g = vec![1u8];
    for i in 0..ec_len as u32 {
        let root = EXP[i as usize];
        let mut next = vec![0u8; g.len() + 1];
        for (j, &c) in g.iter().enumerate() {
            next[j] ^= gmul(c, root);
            next[j + 1] ^= c;
        }
        g = next;
    }
    g
}

/// Systematic RS encoder: `data` plus `ec_len` appended check symbols.
fn rs_encode(data: &[u8], ec_len: usize) -> Vec<u8> {
    let g = rs_generator(ec_len);
    let mut msg = data.to_vec();
    msg.resize(data.len() + ec_len, 0);
    // `msg[i]` is the coefficient of x^{n-1-i}: walk g descending.
    for i in 0..data.len() {
        let coef = msg[i];
        if coef != 0 {
            for (j, &gc) in g.iter().rev().enumerate() {
                msg[i + j] ^= gmul(coef, gc);
            }
        }
    }
    let mut out = data.to_vec();
    out.extend_from_slice(&msg[data.len()..]);
    out
}

// ------------------------------------------------------- spec constants

/// Error-correction level (format-info bit encoding).
#[derive(Clone, Copy)]
enum Ec {
    L,
    M,
    Q,
    H,
}

impl Ec {
    const fn bits(self) -> u8 {
        match self {
            Self::L => 1,
            Self::M => 0,
            Self::Q => 3,
            Self::H => 2,
        }
    }
}

struct Group {
    count: usize,
    data_len: usize,
}

/// (ec_len, [(count, data_len); ≤2]) per (version-1, level).
fn ec_structure(version: u8, level: Ec) -> (usize, Vec<Group>) {
    use Ec::{H, L, M, Q};
    let (e, gs): (usize, &[(usize, usize)]) = match (version, level) {
        (1, L) => (7, &[(1, 19)]),
        (1, M) => (10, &[(1, 16)]),
        (1, Q) => (13, &[(1, 13)]),
        (1, H) => (17, &[(1, 9)]),
        (2, L) => (10, &[(1, 34)]),
        (2, M) => (16, &[(1, 28)]),
        (2, Q) => (22, &[(1, 22)]),
        (2, H) => (28, &[(1, 16)]),
        (3, L) => (15, &[(1, 55)]),
        (3, M) => (26, &[(1, 44)]),
        (3, Q) => (18, &[(2, 17)]),
        (3, H) => (22, &[(2, 13)]),
        (4, L) => (20, &[(1, 80)]),
        (4, M) => (18, &[(2, 32)]),
        (4, Q) => (26, &[(2, 24)]),
        (4, H) => (16, &[(4, 9)]),
        (5, L) => (26, &[(1, 108)]),
        (5, M) => (24, &[(2, 43)]),
        (5, Q) => (18, &[(2, 15), (2, 16)]),
        (5, H) => (22, &[(2, 11), (2, 12)]),
        (6, L) => (18, &[(2, 68)]),
        (6, M) => (16, &[(4, 27)]),
        (6, Q) => (24, &[(4, 19)]),
        (6, H) => (28, &[(4, 15)]),
        (7, L) => (20, &[(2, 78)]),
        (7, M) => (18, &[(4, 31)]),
        (7, Q) => (18, &[(2, 14), (4, 15)]),
        (7, H) => (26, &[(4, 13), (1, 14)]),
        (8, L) => (24, &[(2, 97)]),
        (8, M) => (22, &[(2, 38), (2, 39)]),
        (8, Q) => (22, &[(4, 18), (2, 19)]),
        (8, H) => (26, &[(4, 14), (2, 15)]),
        (9, L) => (30, &[(2, 116)]),
        (9, M) => (22, &[(3, 36), (2, 37)]),
        (9, Q) => (20, &[(4, 16), (4, 17)]),
        (9, H) => (24, &[(4, 12), (4, 13)]),
        (10, L) => (18, &[(2, 68), (2, 69)]),
        (10, M) => (26, &[(4, 43), (1, 44)]),
        (10, Q) => (24, &[(6, 19), (2, 20)]),
        (10, H) => (28, &[(6, 15), (2, 16)]),
        _ => panic!("unsupported version {version}"),
    };
    (
        e,
        gs.iter()
            .map(|&(count, data_len)| Group { count, data_len })
            .collect(),
    )
}

fn alignment_centers(version: u8) -> &'static [u8] {
    match version {
        1 => &[],
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

fn mask_bit(mask: u8, row: u32, col: u32) -> bool {
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

const fn bch_remainder(mut v: u32, g: u32) -> u32 {
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

const fn format_word(data: u32) -> u32 {
    let v = data << 10;
    (v | bch_remainder(v, 0b101_0011_0111)) ^ 0b101_0100_0001_0010
}

const fn version_word(v: u32) -> u32 {
    let s = v << 12;
    s | bch_remainder(s, 0b1_1111_0010_0101)
}

// ------------------------------------------------------------- encoding

struct BitBuf {
    bits: Vec<bool>,
}

impl BitBuf {
    fn new() -> Self {
        Self { bits: Vec::new() }
    }
    fn push(&mut self, v: u32, n: usize) {
        for i in (0..n).rev() {
            self.bits.push((v >> i) & 1 == 1);
        }
    }
    fn len(&self) -> usize {
        self.bits.len()
    }
    fn bytes(&self) -> Vec<u8> {
        self.bits
            .chunks(8)
            .map(|c| {
                let mut b = 0u8;
                for &bit in c {
                    b = (b << 1) | u8::from(bit);
                }
                b << (8 - c.len())
            })
            .collect()
    }
}

/// A single segment to encode.
enum Segment {
    Numeric(String),
    Alphanumeric(String),
    Byte(Vec<u8>),
    Fnc1,
}

fn char_count_bits(mode: u8, version: u8) -> usize {
    let group = if version <= 9 { 0 } else { 1 };
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

const ALNUM: &[u8; 45] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";

fn alnum_value(c: u8) -> u32 {
    ALNUM.iter().position(|&a| a == c).unwrap() as u32
}

/// Full symbol matrix for `segments` at (version, level, mask).
fn encode_matrix(version: u8, level: Ec, segments: &[Segment], mask: u8) -> Vec<Vec<bool>> {
    let (ec_len, groups) = ec_structure(version, level);
    let data_cap: usize = groups.iter().map(|g| g.count * g.data_len).sum();
    let total: usize = groups.iter().map(|g| g.count * (g.data_len + ec_len)).sum();

    // --- segment bitstream ---
    let mut bb = BitBuf::new();
    for seg in segments {
        match seg {
            Segment::Numeric(s) => {
                bb.push(1, 4);
                bb.push(s.len() as u32, char_count_bits(1, version));
                let digits: Vec<u8> = s.bytes().map(|b| b - b'0').collect();
                let mut i = 0;
                while i < digits.len() {
                    let g = (digits.len() - i).min(3);
                    let mut v = 0u32;
                    for &d in &digits[i..i + g] {
                        v = v * 10 + u32::from(d);
                    }
                    bb.push(v, if g == 3 { 10 } else { g * 3 + 1 });
                    i += g;
                }
            }
            Segment::Alphanumeric(s) => {
                bb.push(2, 4);
                bb.push(s.len() as u32, char_count_bits(2, version));
                let bytes = s.as_bytes();
                let mut i = 0;
                while i < bytes.len() {
                    if i + 1 < bytes.len() {
                        bb.push(45 * alnum_value(bytes[i]) + alnum_value(bytes[i + 1]), 11);
                        i += 2;
                    } else {
                        bb.push(alnum_value(bytes[i]), 6);
                        i += 1;
                    }
                }
            }
            Segment::Byte(bytes) => {
                bb.push(4, 4);
                bb.push(bytes.len() as u32, char_count_bits(4, version));
                for &b in bytes {
                    bb.push(u32::from(b), 8);
                }
            }
            Segment::Fnc1 => bb.push(5, 4), // FNC1 first position
        }
    }
    // Terminator + pad to capacity.
    let cap_bits = data_cap * 8;
    assert!(
        bb.len() <= cap_bits,
        "payload exceeds capacity: {} > {}",
        bb.len(),
        cap_bits
    );
    let term = (cap_bits - bb.len()).min(4);
    bb.push(0, term);
    while !bb.len().is_multiple_of(8) {
        bb.push(0, 1);
    }
    let mut data = bb.bytes();
    let mut pad = true;
    while data.len() < data_cap {
        data.push(if pad { 0xEC } else { 0x11 });
        pad = !pad;
    }

    // --- blocks + RS + interleave ---
    let mut blocks: Vec<Vec<u8>> = Vec::new();
    let mut pos = 0usize;
    for g in &groups {
        for _ in 0..g.count {
            let d = &data[pos..pos + g.data_len];
            blocks.push(rs_encode(d, ec_len));
            pos += g.data_len;
        }
    }
    let mut codewords = Vec::with_capacity(total);
    for i in 0..groups.iter().map(|g| g.data_len).max().unwrap_or(0) {
        for block in &blocks {
            let dlen = block.len() - ec_len;
            if i < dlen {
                codewords.push(block[i]);
            }
        }
    }
    for i in 0..ec_len {
        for block in &blocks {
            codewords.push(block[block.len() - ec_len + i]);
        }
    }
    assert_eq!(codewords.len(), total);

    // --- matrix ---
    let dim = 17 + 4 * version as usize;
    let mut m = vec![vec![false; dim]; dim];
    let mut func = vec![vec![false; dim]; dim];
    let mut region = |x0: i32, y0: i32, w: i32, h: i32| {
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                if x >= 0 && y >= 0 && (x as usize) < dim && (y as usize) < dim {
                    func[y as usize][x as usize] = true;
                }
            }
        }
    };

    // Timing (painted before finders so the finders overwrite it).
    for i in 0..dim {
        m[6][i] = i % 2 == 0;
        m[i][6] = i % 2 == 0;
    }
    region(0, 6, dim as i32, 1);
    region(6, 0, 1, dim as i32);

    // Finder patterns with separators.
    let mut finder = |ox: usize, oy: usize| {
        for dy in 0..7 {
            for dx in 0..7 {
                let border = dy == 0 || dy == 6 || dx == 0 || dx == 6;
                let core = (2..=4).contains(&dx) && (2..=4).contains(&dy);
                m[oy + dy][ox + dx] = border || core;
            }
        }
    };
    finder(0, 0);
    finder(dim - 7, 0);
    finder(0, dim - 7);
    region(0, 0, 9, 9);
    region(dim as i32 - 8, 0, 8, 9);
    region(0, dim as i32 - 8, 9, 8);

    // Alignment patterns.
    let centers = alignment_centers(version);
    let last = *centers.last().unwrap_or(&0);
    for &cx in centers {
        for &cy in centers {
            let at_finder =
                (cx == 6 && cy == 6) || (cx == 6 && cy == last) || (cx == last && cy == 6);
            if at_finder {
                continue;
            }
            let (cx, cy) = (cx as usize, cy as usize);
            for dy in 0..5 {
                for dx in 0..5 {
                    m[cy - 2 + dy][cx - 2 + dx] =
                        dy == 0 || dy == 4 || dx == 0 || dx == 4 || (dx == 2 && dy == 2);
                }
            }
            region(cx as i32 - 2, cy as i32 - 2, 5, 5);
        }
    }

    // Dark module + format-information copies.
    m[4 * version as usize + 9][8] = true;
    let fmt = format_word(u32::from(level.bits() << 3 | mask));
    let copy1_pos: [(usize, usize); 15] = [
        (0, 8),
        (1, 8),
        (2, 8),
        (3, 8),
        (4, 8),
        (5, 8),
        (7, 8),
        (8, 8),
        (8, 7),
        (8, 5),
        (8, 4),
        (8, 3),
        (8, 2),
        (8, 1),
        (8, 0),
    ];
    let mut copy2_pos: [(usize, usize); 15] = [(0, 0); 15];
    for k in 0..7 {
        copy2_pos[k] = (8, dim - 1 - k);
    }
    for k in 0..8 {
        copy2_pos[7 + k] = (dim - 8 + k, 8);
    }
    for k in 0..15 {
        let bit = (fmt >> (14 - k)) & 1 == 1;
        let (x1, y1) = copy1_pos[k];
        let (x2, y2) = copy2_pos[k];
        m[y1][x1] = bit;
        m[y2][x2] = bit;
    }

    // Version information (v >= 7), both copies.
    if version >= 7 {
        let vw = version_word(u32::from(version));
        let mut k = 0;
        for j in (0..6).rev() {
            for i in (dim - 11..=dim - 9).rev() {
                let bit = (vw >> (17 - k)) & 1 == 1;
                m[j][i] = bit;
                m[i][j] = bit;
                k += 1;
            }
        }
        region(dim as i32 - 11, 0, 3, 6);
        region(0, dim as i32 - 11, 6, 3);
    }

    // Data placement: zigzag, MSB-first, masked.
    let mut bit_idx = 0usize;
    let n_bits = codewords.len() * 8;
    let bit_at = |i: usize| -> bool { i < n_bits && (codewords[i / 8] >> (7 - i % 8)) & 1 == 1 };
    let mut col = dim as i32 - 1;
    let mut up = true;
    while col > 0 {
        if col == 6 {
            col -= 1;
        }
        for i in 0..dim as i32 {
            let row = if up { dim as i32 - 1 - i } else { i };
            for c in [col, col - 1] {
                if func[row as usize][c as usize] {
                    continue;
                }
                let mut v = bit_at(bit_idx);
                bit_idx += 1;
                if mask_bit(mask, row as u32, c as u32) {
                    v = !v;
                }
                m[row as usize][c as usize] = v;
            }
        }
        up = !up;
        col -= 2;
    }
    m
}

// ------------------------------------------------------------ rendering

/// Render a matrix to a luma buffer: `module` px modules, `quiet`
/// modules of margin on every side.
fn render(m: &[Vec<bool>], module: u32, quiet: u32) -> (Vec<u8>, u32, u32) {
    let dim = m.len() as u32;
    let side = (dim + 2 * quiet) * module;
    let mut img = vec![200u8; (side * side) as usize];
    for (y, row) in m.iter().enumerate() {
        for (x, &dark) in row.iter().enumerate() {
            if !dark {
                continue;
            }
            for dy in 0..module {
                for dx in 0..module {
                    let px = (quiet + x as u32) * module + dx;
                    let py = (quiet + y as u32) * module + dy;
                    img[(py * side + px) as usize] = 20;
                }
            }
        }
    }
    (img, side, side)
}

fn frame(data: &[u8], w: u32, h: u32) -> CpuFrame<'_> {
    CpuFrame::luma(data, w, h, w as usize).unwrap()
}

/// Encode+render helper: v1 by default unless `version` given.
fn qr_image(
    version: u8,
    level: Ec,
    segments: &[Segment],
    mask: u8,
    module: u32,
    quiet: u32,
) -> (Vec<u8>, u32, u32) {
    render(
        &encode_matrix(version, level, segments, mask),
        module,
        quiet,
    )
}

// ----------------------------------------------------------------- tests

#[test]
fn decodes_numeric_v1() {
    let payload = "0123456789012345";
    let (img, w, h) = qr_image(1, Ec::L, &[Segment::Numeric(payload.into())], 0, 4, 6);
    let codes = BarcodeEngine::new().decode(&frame(&img, w, h)).unwrap();
    assert_eq!(codes.len(), 1, "expected one barcode, got {codes:?}");
    let c = &codes[0];
    assert_eq!(c.symbology, Symbology::QrCode);
    assert_eq!(c.text, payload);
    assert_eq!(c.raw, payload.as_bytes());
    assert!(c.confidence > 0.5);
}

#[test]
fn decodes_alphanumeric_v1() {
    let payload = "HELLO WORLD $%*+-./:";
    let (img, w, h) = qr_image(1, Ec::M, &[Segment::Alphanumeric(payload.into())], 1, 4, 6);
    let codes = BarcodeEngine::new().decode(&frame(&img, w, h)).unwrap();
    assert_eq!(codes.len(), 1, "got {codes:?}");
    assert_eq!(codes[0].text, payload);
}

#[test]
fn decodes_byte_mode_with_nonzero_mask() {
    // v3-Q: two RS blocks, 34 data codewords.
    let payload = b"Byte mode, mask 3: \xC3\xA9";
    let (img, w, h) = qr_image(3, Ec::Q, &[Segment::Byte(payload.to_vec())], 3, 4, 6);
    let report = BarcodeEngine::new()
        .decode_report(&frame(&img, w, h))
        .unwrap();
    assert_eq!(report.barcodes.len(), 1, "attempts: {:?}", report.attempts);
    assert_eq!(report.barcodes[0].raw, payload);
    // é is valid UTF-8, so text matches the UTF-8 interpretation.
    assert_eq!(report.barcodes[0].text, "Byte mode, mask 3: é");
}

#[test]
fn decodes_version7_with_version_info() {
    let payload = b"Version 7 exercises the 18-bit version information path.";
    let (img, w, h) = qr_image(7, Ec::L, &[Segment::Byte(payload.to_vec())], 4, 4, 6);
    let report = BarcodeEngine::new()
        .decode_report(&frame(&img, w, h))
        .unwrap();
    assert_eq!(report.barcodes.len(), 1, "attempts: {:?}", report.attempts);
    assert_eq!(report.barcodes[0].raw, payload);
}

#[test]
fn decodes_mixed_segments() {
    let segs = [
        Segment::Numeric("31415926".into()),
        Segment::Alphanumeric(" PI-".into()),
        Segment::Byte(b"3.14159".to_vec()),
    ];
    let (img, w, h) = qr_image(3, Ec::M, &segs, 5, 4, 6);
    let codes = BarcodeEngine::new().decode(&frame(&img, w, h)).unwrap();
    assert_eq!(codes.len(), 1, "got {codes:?}");
    assert_eq!(codes[0].text, "31415926 PI-3.14159");
}

#[test]
fn decodes_rotated_90_degrees() {
    let payload = "ROTATED QR";
    let (img, w, h) = qr_image(1, Ec::L, &[Segment::Alphanumeric(payload.into())], 0, 4, 6);
    // Rotate 90° clockwise: dst(dx, dy) = src(w - 1 - dy, dx).
    let mut rot = vec![200u8; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let dx = h - 1 - y;
            let dy = x;
            rot[(dy * h + dx) as usize] = img[(y * w + x) as usize];
        }
    }
    let codes = BarcodeEngine::new().decode(&frame(&rot, h, w)).unwrap();
    assert_eq!(codes.len(), 1, "rotated code not decoded: {codes:?}");
    assert_eq!(codes[0].text, payload);
}

#[test]
fn inverted_option_decodes_reversed_contrast() {
    let payload = b"inverted qr";
    let (img, w, h) = qr_image(1, Ec::M, &[Segment::Byte(payload.to_vec())], 2, 4, 6);
    let inv: Vec<u8> = img.iter().map(|&p| 255 - p).collect();
    let engine = BarcodeEngine::new().with_options(DecodeOptions {
        inverted: true,
        axes: ScanAxes::Both,
    });
    let codes = engine.decode(&frame(&inv, w, h)).unwrap();
    assert_eq!(codes.len(), 1, "got {codes:?}");
    assert_eq!(codes[0].raw, payload);
    // Without the inverted option the reversed frame must not decode.
    let none = BarcodeEngine::new().decode(&frame(&inv, w, h)).unwrap();
    assert!(none.is_empty());
}

#[test]
fn rejects_data_beyond_ec_capacity() {
    let payload = b"corrupt me";
    let mut matrix = encode_matrix(1, Ec::L, &[Segment::Byte(payload.to_vec())], 0);
    // Flip many modules in the codeword region: the L block only
    // corrects 3 symbols, and each flip hits a distinct codeword.
    let dim = matrix.len();
    let mut flipped = 0;
    'outer: for y in 9..dim - 1 {
        for x in 9..dim - 1 {
            if flipped == 24 {
                break 'outer;
            }
            matrix[y][x] = !matrix[y][x];
            flipped += 1;
        }
    }
    let (img, w, h) = render(&matrix, 4, 6);
    let report = BarcodeEngine::new()
        .decode_report(&frame(&img, w, h))
        .unwrap();
    assert!(
        report.barcodes.is_empty(),
        "decoded corrupt data: {report:?}"
    );
    assert!(
        report.attempts.iter().any(|a| matches!(
            a.reason,
            RejectReason::ErrorCorrectionFailed { .. }
                | RejectReason::StructureMismatch { .. }
                | RejectReason::DataUndecodable { .. }
        )),
        "expected a structured rejection, got {:?}",
        report.attempts
    );
}

#[test]
fn rejects_quiet_zone_violation() {
    let payload = b"quiet";
    let matrix = encode_matrix(1, Ec::L, &[Segment::Byte(payload.to_vec())], 0);
    // Render with minimal margin, then paint a dark blob two modules
    // above the symbol inside the quiet zone.
    let module = 4u32;
    let quiet = 4u32;
    let (mut img, w, h) = render(&matrix, module, quiet);
    let blob_top = quiet * module - 2 * module; // 2 modules above symbol
    for y in blob_top..blob_top + module {
        for x in (quiet + 2) * module..(quiet + 4) * module {
            img[(y * w + x) as usize] = 20;
        }
    }
    let report = BarcodeEngine::new()
        .decode_report(&frame(&img, w, h))
        .unwrap();
    assert!(
        report.barcodes.is_empty(),
        "decoded with violated quiet zone"
    );
    assert!(
        report
            .attempts
            .iter()
            .any(|a| matches!(a.reason, RejectReason::QuietZone { .. })),
        "expected quiet-zone diagnostic, got {:?}",
        report.attempts
    );
}

#[test]
fn formats_gate_excludes_qr() {
    let (img, w, h) = qr_image(1, Ec::L, &[Segment::Numeric("0123456789".into())], 0, 4, 6);
    let engine = BarcodeEngine::with_formats(Formats::EAN13);
    let codes = engine.decode(&frame(&img, w, h)).unwrap();
    assert!(codes.is_empty(), "QR decoded when only EAN13 selected");
    // QR alone selected works; ALL also selects it.
    let qr_only = BarcodeEngine::with_formats(Formats::QR)
        .decode(&frame(&img, w, h))
        .unwrap();
    assert_eq!(qr_only.len(), 1);
    assert!(Formats::ALL.contains(Formats::QR));
}

#[test]
fn decode_is_deterministic() {
    let (img, w, h) = qr_image(
        4,
        Ec::H,
        &[Segment::Byte(b"deterministic".to_vec())],
        7,
        4,
        6,
    );
    let f = frame(&img, w, h);
    let a = BarcodeEngine::new().decode_report(&f).unwrap();
    let b = BarcodeEngine::new().decode_report(&f).unwrap();
    assert_eq!(a.barcodes.len(), 1, "attempts: {:?}", a.attempts);
    assert_eq!(a.barcodes.len(), b.barcodes.len());
    assert_eq!(a.barcodes[0].text, b.barcodes[0].text);
    assert_eq!(a.barcodes[0].confidence, b.barcodes[0].confidence);
}

#[test]
fn blank_frame_decodes_nothing() {
    let img = vec![200u8; 120 * 120];
    let report = BarcodeEngine::new()
        .decode_report(&frame(&img, 120, 120))
        .unwrap();
    assert!(report.barcodes.is_empty());
    assert!(report.attempts.is_empty());
}

#[test]
fn decodes_fnc1_percent_markers_and_preserves_byte_segment_raw() {
    // Regression (panic): raw was once sliced by UTF-8 char lengths, so
    // a multi-byte char after '%' read out of bounds and panicked. FNC1
    // '%' substitution belongs to the alphanumeric layer only — the
    // byte segment's literal 0x25 and multi-byte octets stay exact.
    let segs = [
        Segment::Fnc1,
        Segment::Alphanumeric("A%%B%A".into()),
        Segment::Byte("é%".as_bytes().to_vec()),
    ];
    let (img, w, h) = qr_image(1, Ec::L, &segs, 0, 4, 6);
    let codes = BarcodeEngine::new().decode(&frame(&img, w, h)).unwrap();
    assert_eq!(codes.len(), 1, "expected one barcode, got {codes:?}");
    let c = &codes[0];
    assert_eq!(c.text, "A%B\u{001D}Aé%");
    assert_eq!(
        c.raw,
        [b'A', b'%', b'B', 0x1D, b'A', 0xC3, 0xA9, b'%'].as_slice()
    );

    // Deterministic across decodes.
    let f = frame(&img, w, h);
    let again = BarcodeEngine::new().decode(&f).unwrap();
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].text, codes[0].text);
    assert_eq!(again[0].raw, codes[0].raw);
}
