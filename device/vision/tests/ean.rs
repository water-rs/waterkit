//! Integration tests for the EAN-13/UPC-A decoder, driven by a
//! spec-faithful software renderer (no binary fixtures).

#![allow(clippy::cast_possible_truncation, clippy::many_single_char_names)]

use waterkit_vision::{
    BarcodeEngine, CpuFrame, DecodeOptions, Formats, RejectReason, ScanAxes, Symbology,
};

/// L/G patterns for the six left digits (L = 0, G = 1), per digit 0-9,
/// mirroring `FIRST_DIGIT_PARITY` in the decoder.
const FIRST_DIGIT_PARITY: [u8; 10] = [
    0b00_0000, 0b00_1011, 0b00_1101, 0b00_1110, 0b01_0011, 0b01_1001, 0b01_1100, 0b01_0101,
    0b01_0110, 0b01_1010,
];

/// Left-parity bit patterns (7 modules, MSB-first).
const L_BITS: [u8; 10] = [
    0b000_1101, 0b001_1001, 0b001_0011, 0b011_1101, 0b010_0011, 0b011_0001, 0b010_1111, 0b011_1011,
    0b011_0111, 0b000_1011,
];

fn mod10_check(first12: &[u8; 12]) -> u8 {
    let sum: u32 = first12
        .iter()
        .enumerate()
        .map(|(i, &d)| u32::from(d) * if i % 2 == 0 { 1 } else { 3 })
        .sum();
    ((10 - sum % 10) % 10) as u8
}

/// The 95 module columns of the symbol itself (guards + digits), as
/// `(module_index, is_bar)` pairs generated on the fly.
fn symbol_bits(digits: &[u8; 13]) -> [bool; 95] {
    let mut bits = [false; 95];
    let mut m = 0usize;
    // Start guard 101.
    bits[m] = true;
    bits[m + 2] = true;
    m += 3;
    let parity = FIRST_DIGIT_PARITY[digits[0] as usize];
    for (k, &d) in digits[1..7].iter().enumerate() {
        let l = L_BITS[d as usize];
        // G parity = bit-reversed R parity.
        let g = (L_BITS[d as usize] ^ 0x7F).reverse_bits() >> 1;
        let pattern = if (parity >> (5 - k)) & 1 == 1 { g } else { l };
        for (i, bit) in bits[m..m + 7].iter_mut().enumerate() {
            *bit = (pattern >> (6 - i)) & 1 == 1;
        }
        m += 7;
    }
    // Middle guard 01010.
    bits[m + 1] = true;
    bits[m + 3] = true;
    m += 5;
    for &d in &digits[7..13] {
        let r = L_BITS[d as usize] ^ 0x7F;
        for (i, bit) in bits[m..m + 7].iter_mut().enumerate() {
            *bit = (r >> (6 - i)) & 1 == 1;
        }
        m += 7;
    }
    // End guard 101.
    bits[m] = true;
    bits[m + 2] = true;
    bits
}

/// Paint an EAN-13 symbol into an existing Luma8 canvas.
///
/// `module` = module width in px. The symbol's code region starts at
/// `x0` and bars span `y0..y1`; `canvas` must leave `x0 >= 11*module`
/// and `canvas_w - x0 - 95*module >= 7*module` for the quiet zones.
#[allow(clippy::too_many_arguments)]
fn paint_ean13(
    canvas: &mut [u8],
    canvas_w: u32,
    digits: &[u8; 13],
    module: u32,
    x0: u32,
    y0: u32,
    y1: u32,
) {
    let bits = symbol_bits(digits);
    for (i, &bar) in bits.iter().enumerate() {
        if !bar {
            continue;
        }
        let col = x0 + i as u32 * module;
        for y in y0..y1 {
            for x in col..col + module {
                canvas[(y * canvas_w + x) as usize] = 20;
            }
        }
    }
}

const fn render_size(module: u32) -> u32 {
    (11 + 95 + 7) * module
}

fn ean13_image(digits: &[u8; 13]) -> (Vec<u8>, u32, u32) {
    let module = 4;
    let w = render_size(module);
    let h = 64;
    let mut img = vec![200u8; (w * h) as usize];
    paint_ean13(&mut img, w, digits, module, 11 * module, 0, h);
    (img, w, h)
}

fn frame(data: &[u8], w: u32, h: u32) -> CpuFrame<'_> {
    CpuFrame::luma(data, w, h, w as usize).unwrap()
}

fn valid_d13() -> [u8; 13] {
    let mut d = [5, 9, 0, 1, 2, 3, 4, 1, 2, 3, 4, 5, 0];
    d[12] = mod10_check(&d[..12].try_into().unwrap());
    d
}

#[test]
fn decodes_valid_ean13() {
    let (img, w, h) = ean13_image(&valid_d13());
    let codes = BarcodeEngine::new().decode(&frame(&img, w, h)).unwrap();
    assert_eq!(codes.len(), 1, "expected one barcode, got {codes:?}");
    let c = &codes[0];
    assert_eq!(c.symbology, Symbology::Ean13);
    assert_eq!(c.text, "5901234123457");
    assert_eq!(c.raw, c.text.as_bytes());
    assert!(c.confidence > 0.5);
    assert!(c.evidence.rows_matched > 10);
}

#[test]
fn decodes_upca_as_ean13_subset() {
    // UPC-A 036000291452 == EAN-13 0036000291452.
    let digits = [0, 0, 3, 6, 0, 0, 0, 2, 9, 1, 4, 5, 2];
    assert_eq!(mod10_check(&digits[..12].try_into().unwrap()), 2);
    let (img, w, h) = ean13_image(&digits);
    let codes = BarcodeEngine::new().decode(&frame(&img, w, h)).unwrap();
    assert_eq!(codes.len(), 1);
    assert_eq!(codes[0].symbology, Symbology::UpcA);
    assert_eq!(codes[0].text, "036000291452", "leading zero dropped");
}

#[test]
fn rejects_bad_checksum() {
    let digits = [5, 9, 0, 1, 2, 3, 4, 1, 2, 3, 4, 5, 9];
    let (img, w, h) = ean13_image(&digits);
    let report = BarcodeEngine::new()
        .decode_report(&frame(&img, w, h))
        .unwrap();
    assert!(report.barcodes.is_empty());
    assert!(
        report
            .attempts
            .iter()
            .any(|a| matches!(a.reason, RejectReason::ChecksumMismatch { .. })),
        "expected checksum diagnostic, got {:?}",
        report.attempts
    );
}

#[test]
fn rejects_missing_quiet_zone() {
    // Paint flush against the left edge: no observable quiet zone.
    let module = 4;
    let w = render_size(module);
    let h = 64u32;
    let mut img = vec![200u8; (w * h) as usize];
    // Only 2-module margin before the start guard.
    paint_ean13(&mut img, w, &valid_d13(), module, 2 * module, 0, h);
    let report = BarcodeEngine::new()
        .decode_report(&frame(&img, w, h))
        .unwrap();
    assert!(report.barcodes.is_empty());
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
fn decodes_rotated_90_degrees() {
    let (img, w, h) = ean13_image(&valid_d13());
    // Rotate buffer 90° clockwise: dst(dx, dy) = src(w - 1 - dy, dx).
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
    assert_eq!(codes[0].text, "5901234123457");
    // Quad sits in rotated (buffer) space: taller than wide.
    assert!(codes[0].quad.height() > codes[0].quad.width());
}

#[test]
fn orders_multiple_codes_deterministically() {
    let d1 = valid_d13();
    let mut d2 = [4, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0];
    d2[12] = mod10_check(&d2[..12].try_into().unwrap());
    let module = 4;
    let w = render_size(module);
    let h = 96u32;
    let mut img = vec![200u8; (w * h) as usize];
    paint_ean13(&mut img, w, &d1, module, 11 * module, 8, 44);
    paint_ean13(&mut img, w, &d2, module, 11 * module, 56, 92);
    let codes = BarcodeEngine::new().decode(&frame(&img, w, h)).unwrap();
    assert_eq!(codes.len(), 2, "got {codes:?}");
    assert_eq!(codes[0].text, "5901234123457", "top code first");
    let expected2: String = d2.iter().map(u8::to_string).collect();
    assert_eq!(codes[1].text, expected2);
}

#[test]
fn inverted_option_decodes_light_bars() {
    let (img, w, h) = ean13_image(&valid_d13());
    let inv: Vec<u8> = img.iter().map(|&p| 255 - p).collect();
    let engine = BarcodeEngine::new().with_options(DecodeOptions {
        inverted: true,
        axes: ScanAxes::Horizontal,
    });
    let codes = engine.decode(&frame(&inv, w, h)).unwrap();
    assert_eq!(codes.len(), 1);
    assert_eq!(codes[0].text, "5901234123457");
}

#[test]
fn formats_gate_excludes_symbology() {
    let (img, w, h) = ean13_image(&valid_d13());
    let engine = BarcodeEngine::with_formats(Formats::UPCA);
    let codes = engine.decode(&frame(&img, w, h)).unwrap();
    assert!(
        codes.is_empty(),
        "EAN-13 filtered out when only UPCA selected"
    );
}

#[test]
fn blank_frame_decodes_nothing() {
    let img = vec![200u8; 100 * 60];
    let codes = BarcodeEngine::new().decode(&frame(&img, 100, 60)).unwrap();
    assert!(codes.is_empty());
}

#[test]
fn qr_only_formats_skip_ean_scanline_diagnostics() {
    // Issue #125: a QR-only selection must not run the EAN scanline
    // path at all — no EAN rejects in the report, no EAN CPU spent.
    // A checksum-bad row is what makes the leak observable: ungated,
    // every scanned row would record a ChecksumMismatch attempt.
    let digits = [5, 9, 0, 1, 2, 3, 4, 1, 2, 3, 4, 5, 9];
    let (img, w, h) = ean13_image(&digits);
    let report = BarcodeEngine::with_formats(Formats::QR)
        .decode_report(&frame(&img, w, h))
        .unwrap();
    assert!(report.barcodes.is_empty());
    assert!(
        report
            .attempts
            .iter()
            .all(|a| a.symbology == Symbology::QrCode),
        "EAN attempts leaked into a QR-only report: {:?}",
        report.attempts
    );
}
