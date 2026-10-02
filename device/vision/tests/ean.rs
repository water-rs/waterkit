//! Integration tests for the EAN-13/UPC-A decoder, driven by a
//! spec-faithful software renderer (no binary fixtures).

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
    0b000_1101, 0b001_1001, 0b001_0011, 0b011_1101, 0b010_0011, 0b011_0001, 0b010_1111,
    0b011_1011, 0b011_0111, 0b000_1011,
];

fn mod10_check(first12: &[u8; 12]) -> u8 {
    let sum: u32 = first12
        .iter()
        .enumerate()
        .map(|(i, &d)| u32::from(d) * if i % 2 == 0 { 1 } else { 3 })
        .sum();
    ((10 - sum % 10) % 10) as u8
}

/// Render an EAN-13 symbol into a Luma8 buffer.
///
/// `module` is the module width in pixels; quiet zones follow spec
/// (11 left / 7 right). Bars span `bar_top..bar_bottom` rows.
fn render_ean13(
    digits: &[u8; 13],
    module: u32,
    bar_top: u32,
    bar_bottom: u32,
    quiet_left: u32,
    quiet_right: u32,
) -> (Vec<u8>, u32, u32) {
    // Total modules: 11 + 3 + 42 + 5 + 42 + 3 + 7 = 113.
    let modules = 113u32;
    let w = modules * module;
    let h = (bar_bottom - bar_top).max(8);
    let mut img = vec![200u8; (w * h) as usize];

    // Module-column -> bit sequence (true = bar).
    let mut bits = vec![false; modules as usize];
    let mut m = 0usize;
    // Start guard.
    bits[m] = true;
    bits[m + 2] = true;
    m += 3;
    let parity = FIRST_DIGIT_PARITY[digits[0] as usize];
    for (k, &d) in digits[1..7].iter().enumerate() {
        let l = L_BITS[d as usize];
        // G parity = complement; note runs/bits layout: G bits are the
        // mirror-image (reversed R) — emit reversed R bits.
        let g = (L_BITS[d as usize] ^ 0x7F).reverse_bits() >> 1;
        let pattern = if (parity >> (5 - k)) & 1 == 1 { g } else { l };
        for i in 0..7 {
            bits[m + i] = (pattern >> (6 - i)) & 1 == 1;
        }
        m += 7;
    }
    // Middle guard 01010.
    bits[m + 1] = true;
    bits[m + 3] = true;
    m += 5;
    for &d in &digits[7..13] {
        let r = L_BITS[d as usize] ^ 0x7F;
        for i in 0..7 {
            bits[m + i] = (r >> (6 - i)) & 1 == 1;
        }
        m += 7;
    }
    bits[m] = true;
    bits[m + 2] = true;

    // Paint: modules after `quiet_left/module` are code; outside is quiet.
    let code_modules = 95u32; // 3+42+5+42+3
    for (i, &b) in bits.iter().enumerate() {
        if !b {
            continue;
        }
        let col = i as u32;
        // Only paint the symbol region; quiet zone bits are never set.
        if col < quiet_left / module || col >= quiet_left / module + code_modules {
            continue;
        }
        for y in bar_top..bar_bottom {
            for x in col * module..(col + 1) * module {
                img[(y * w + x) as usize] = 20;
            }
        }
    }
    (img, w, h)
}

/// Standard render: full quiet zones, bars covering the whole height.
fn ean13_image(digits: &[u8; 13]) -> (Vec<u8>, u32, u32) {
    let module = 4;
    render_ean13(digits, module, 0, 64, 11 * module, 7 * module)
}

fn frame(data: &[u8], w: u32, h: u32) -> CpuFrame<'_> {
    CpuFrame::luma(data, w, h, w as usize).unwrap()
}

#[test]
fn decodes_valid_ean13() {
    let mut digits = [5, 9, 0, 1, 2, 3, 4, 1, 2, 3, 4, 5, 0];
    digits[12] = mod10_check(&digits[..12].try_into().unwrap());
    let (img, w, h) = ean13_image(&digits);
    let engine = BarcodeEngine::new();
    let codes = engine.decode(&frame(&img, w, h)).unwrap();
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
    let digits = [0, 3, 6, 0, 0, 0, 2, 9, 1, 4, 5, 2, 2];
    assert_eq!(mod10_check(&digits[..12].try_into().unwrap()), 2);
    let (img, w, h) = ean13_image(&digits);
    let codes = BarcodeEngine::new().decode(&frame(&img, w, h)).unwrap();
    assert_eq!(codes.len(), 1);
    assert_eq!(codes[0].symbology, Symbology::UpcA);
    assert_eq!(codes[0].text, "036000291452", "leading zero dropped");
}

#[test]
fn rejects_bad_checksum() {
    // Mutate the check digit of a valid code.
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
    // Render with a left quiet zone far below the 11-module minimum.
    let mut digits = [5, 9, 0, 1, 2, 3, 4, 1, 2, 3, 4, 5, 0];
    digits[12] = mod10_check(&digits[..12].try_into().unwrap());
    let module = 4;
    let (img, w, h) = render_ean13(&digits, module, 0, 64, 2 * module, 7 * module);
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
    let mut digits = [5, 9, 0, 1, 2, 3, 4, 1, 2, 3, 4, 5, 0];
    digits[12] = mod10_check(&digits[..12].try_into().unwrap());
    let (img, w, h) = ean13_image(&digits);
    // Rotate buffer 90° clockwise: out[y][x] = in[x][w-1-y].
    let mut rot = vec![200u8; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            rot[(x * h + (w - 1 - y)) as usize] = img[(y * w + x) as usize];
        }
    }
    let codes = BarcodeEngine::new().decode(&frame(&rot, h, w)).unwrap();
    assert_eq!(codes.len(), 1);
    assert_eq!(codes[0].text, "5901234123457");
    // Quad sits in rotated (buffer) space: taller than wide.
    assert!(codes[0].quad.height() > codes[0].quad.width());
}

#[test]
fn orders_multiple_codes_deterministically() {
    let mut d1 = [5, 9, 0, 1, 2, 3, 4, 1, 2, 3, 4, 5, 0];
    d1[12] = mod10_check(&d1[..12].try_into().unwrap());
    let mut d2 = [4, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0];
    d2[12] = mod10_check(&d2[..12].try_into().unwrap());
    // Two symbols stacked vertically in one frame.
    let module = 4;
    let (mut img, w, _h) = render_ean13(d1, module, 8, 48, 11 * module, 7 * module);
    let mut img2 = vec![200u8; (w * 96) as usize];
    img2[..img.len()].copy_from_slice(&img);
    img.append(&mut img2[w as usize * 48..].to_vec());
    let (img_bottom, _, _) = render_ean13(d2, module, 0, 40, 11 * module, 7 * module);
    // Place second symbol at rows 56..96.
    for y in 0..40u32 {
        for x in 0..w {
            img[((56 + y) * w + x) as usize] = img_bottom[(y * w + x) as usize];
        }
    }
    let codes = BarcodeEngine::new().decode(&frame(&img, w, 96)).unwrap();
    assert_eq!(codes.len(), 2);
    assert_eq!(codes[0].text, "5901234123457", "top code first");
    assert_eq!(codes[1].text, "4001234567899".replace("99", ""), "");
    let t2: String = codes[1].text.clone();
    assert_eq!(t2, String::from_iter(d2.iter().map(u8::to_string).collect::<Vec<_>>()));
}

#[test]
fn inverted_option_decodes_light_bars() {
    let mut digits = [5, 9, 0, 1, 2, 3, 4, 1, 2, 3, 4, 5, 0];
    digits[12] = mod10_check(&digits[..12].try_into().unwrap());
    let (img, w, h) = ean13_image(&digits);
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
    let mut digits = [5, 9, 0, 1, 2, 3, 4, 1, 2, 3, 4, 5, 0];
    digits[12] = mod10_check(&digits[..12].try_into().unwrap());
    let (img, w, h) = ean13_image(&digits);
    let engine = BarcodeEngine::with_formats(Formats::UPCA);
    let codes = engine.decode(&frame(&img, w, h)).unwrap();
    assert!(codes.is_empty(), "EAN-13 filtered out when only UPCA selected");
}

#[test]
fn blank_frame_decodes_nothing() {
    let img = vec![200u8; 100 * 60];
    let codes = BarcodeEngine::new().decode(&frame(&img, 100, 60)).unwrap();
    assert!(codes.is_empty());
}
