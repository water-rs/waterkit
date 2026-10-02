//! QR data path: unmasked codeword reading, block deinterleaving,
//! Reed–Solomon correction and segment decoding.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::too_many_lines,
    clippy::missing_const_for_fn
)]

use super::gf;
use super::tables::{self, EcStructure};
use super::{RejectReason, Stage};
use crate::image::BitImage;

/// Byte interpretation applied to byte-mode segments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Charset {
    /// ISO/IEC 8859-1, the spec's default byte interpretation.
    Latin1,
    /// UTF-8.
    Utf8,
    /// UTF-16BE (ECI 25).
    Utf16Be,
}

/// Sequential bit reader over the codeword stream (MSB first).
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len() * 8 - self.pos
    }

    fn read(&mut self, n: usize) -> Option<u32> {
        if n == 0 || self.remaining() < n || n > 32 {
            return None;
        }
        let mut v = 0u32;
        for _ in 0..n {
            let byte = self.data[self.pos / 8];
            let bit = (byte >> (7 - self.pos % 8)) & 1;
            v = (v << 1) | u32::from(bit);
            self.pos += 1;
        }
        Some(v)
    }
}

/// Read the `total` codewords out of the sampled matrix: two-column
/// zigzag from the bottom-right, skipping function modules and column
/// 6, applying `mask` to data cells.
pub fn read_codewords(
    matrix: &BitImage,
    func: &BitImage,
    dim: u32,
    mask: u8,
    total: usize,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(total);
    let mut byte = 0u8;
    let mut filled = 0u8;
    let mut reading_up = true;
    let mut col = dim - 1;
    while col > 0 && out.len() < total {
        if col == 6 {
            col -= 1;
        }
        for i in 0..dim {
            let row = if reading_up { dim - 1 - i } else { i };
            for c in [col, col - 1] {
                if func.get(c, row) {
                    continue;
                }
                let mut bit = matrix.get(c, row);
                if tables::mask_bit(mask, row, c) {
                    bit = !bit;
                }
                byte = (byte << 1) | u8::from(bit);
                filled += 1;
                if filled == 8 {
                    out.push(byte);
                    byte = 0;
                    filled = 0;
                }
            }
        }
        reading_up = !reading_up;
        col = col.saturating_sub(2);
    }
    out
}

/// Deinterleave `codewords` into blocks per `structure`, correct each
/// block with Reed–Solomon, and return `(data_bytes, corrected_errors)`.
///
/// Fails with [`RejectReason::ErrorCorrectionFailed`] when a block
/// exceeds its correction capacity.
pub fn correct_codewords(
    codewords: &[u8],
    structure: &EcStructure,
) -> Result<(Vec<u8>, u32), RejectReason> {
    let mut blocks: Vec<Vec<u8>> = Vec::new();
    for group in structure.groups {
        for _ in 0..group.count {
            blocks.push(vec![0u8; group.data_len + structure.ec_len]);
        }
    }
    let n_blocks = blocks.len();
    // Column-wise data codewords, then column-wise EC codewords.
    let mut pos = 0usize;
    let max_data = blocks
        .iter()
        .map(|b| b.len() - structure.ec_len)
        .max()
        .unwrap_or(0);
    for i in 0..max_data {
        for block in &mut blocks {
            let dlen = block.len() - structure.ec_len;
            if i < dlen && pos < codewords.len() {
                block[i] = codewords[pos];
                pos += 1;
            }
        }
    }
    for i in 0..structure.ec_len {
        for block in &mut blocks {
            let dlen = block.len() - structure.ec_len;
            if pos < codewords.len() {
                block[dlen + i] = codewords[pos];
                pos += 1;
            }
        }
    }

    let mut errors = 0u32;
    for block in &mut blocks {
        match gf::correct(block, structure.ec_len) {
            Some(n) => errors += n,
            None => {
                return Err(RejectReason::ErrorCorrectionFailed {
                    blocks: n_blocks as u8,
                });
            }
        }
    }

    let mut data = Vec::with_capacity(structure.data_len());
    for block in &blocks {
        data.extend_from_slice(&block[..block.len() - structure.ec_len]);
    }
    Ok((data, errors))
}

/// Parse the segment stream: numeric, alphanumeric and byte modes, plus
/// ECI, structured-append and FNC1 headers.
///
/// Returns `(text, raw)` — `text` is the interpreted payload and `raw`
/// the bytes as decoded (byte segments keep their exact octets).
pub fn parse_segments(data: &[u8], version: u8) -> Result<(String, Vec<u8>), RejectReason> {
    let mut reader = BitReader::new(data);
    let mut text = String::new();
    let mut raw: Vec<u8> = Vec::new();
    let mut charset = Charset::Latin1;
    let mut utf8_default = true; // try UTF-8 before Latin-1 until ECI pins one
    let mut fnc1 = false;

    loop {
        if reader.remaining() < 4 {
            break; // terminator shorter than 4 bits: end of stream
        }
        let mode_bit = reader.pos;
        let mode = reader.read(4).unwrap_or(0) as u8;
        match mode {
            0 => break,
            1 => {
                // Numeric.
                let bits = tables::char_count_bits(1, version) as usize;
                let count = reader.read(bits).ok_or(RejectReason::Truncated {
                    stage: Stage::Segments,
                })?;
                let mut i = 0u32;
                while i < count {
                    let group = (count - i).min(3);
                    let width = if group == 3 { 10 } else { group * 3 + 1 };
                    let v = reader.read(width as usize).ok_or(RejectReason::Truncated {
                        stage: Stage::Segments,
                    })?;
                    if v >= 10u32.pow(group) {
                        return Err(RejectReason::DataUndecodable {
                            bit: reader.pos as u32,
                        });
                    }
                    for d in (0..group).rev() {
                        let place = 10u32.pow(d);
                        let digit = v / place % 10;
                        text.push((b'0' + digit as u8) as char);
                        raw.push(b'0' + digit as u8);
                    }
                    i += group;
                }
            }
            2 => {
                // Alphanumeric.
                let bits = tables::char_count_bits(2, version) as usize;
                let count = reader.read(bits).ok_or(RejectReason::Truncated {
                    stage: Stage::Segments,
                })?;
                let mut seg_text = String::new();
                let mut seg_raw = Vec::new();
                let mut i = 0u32;
                while i < count {
                    if count - i >= 2 {
                        let v = reader.read(11).ok_or(RejectReason::Truncated {
                            stage: Stage::Segments,
                        })?;
                        let (a, b) = (v / 45, v % 45);
                        if a >= 45 || b >= 45 {
                            return Err(RejectReason::DataUndecodable {
                                bit: reader.pos as u32,
                            });
                        }
                        for c in [a, b] {
                            let ch = tables::ALPHANUMERIC[c as usize];
                            seg_text.push(ch as char);
                            seg_raw.push(ch);
                        }
                        i += 2;
                    } else {
                        let v = reader.read(6).ok_or(RejectReason::Truncated {
                            stage: Stage::Segments,
                        })?;
                        if v >= 45 {
                            return Err(RejectReason::DataUndecodable {
                                bit: reader.pos as u32,
                            });
                        }
                        let ch = tables::ALPHANUMERIC[v as usize];
                        seg_text.push(ch as char);
                        seg_raw.push(ch);
                        i += 1;
                    }
                }
                // The '%'→GS substitution belongs to the alphanumeric
                // alphabet only; byte-segment octets stay exact.
                if fnc1 {
                    apply_fnc1(&mut seg_text, &mut seg_raw);
                }
                text.push_str(&seg_text);
                raw.extend_from_slice(&seg_raw);
            }
            4 => {
                // Byte mode.
                let bits = tables::char_count_bits(4, version) as usize;
                let count = reader.read(bits).ok_or(RejectReason::Truncated {
                    stage: Stage::Segments,
                })? as usize;
                let mut bytes = Vec::with_capacity(count);
                for _ in 0..count {
                    bytes.push(reader.read(8).ok_or(RejectReason::Truncated {
                        stage: Stage::Segments,
                    })? as u8);
                }
                let decoded = decode_bytes(&bytes, charset, utf8_default, mode_bit)?;
                text.push_str(&decoded);
                raw.extend_from_slice(&bytes);
                utf8_default = false;
            }
            7 => {
                // ECI: variable-length assignment number.
                let first = reader.read(8).ok_or(RejectReason::Truncated {
                    stage: Stage::Segments,
                })?;
                let value = if first & 0x80 == 0 {
                    first & 0x7F
                } else if first & 0xC0 == 0x80 {
                    let rest = reader.read(8).ok_or(RejectReason::Truncated {
                        stage: Stage::Segments,
                    })?;
                    ((first & 0x3F) << 8) | rest
                } else if first & 0xE0 == 0xC0 {
                    let rest = reader.read(16).ok_or(RejectReason::Truncated {
                        stage: Stage::Segments,
                    })?;
                    ((first & 0x1F) << 16) | rest
                } else {
                    return Err(RejectReason::DataUndecodable {
                        bit: mode_bit as u32,
                    });
                };
                charset = match value {
                    20 | 26 => Charset::Utf8,
                    25 => Charset::Utf16Be,
                    // 3 (ISO 8859-1) and unknown assignments keep bytes exact.
                    _ => Charset::Latin1,
                };
                utf8_default = false;
            }
            3 => {
                // Structured append header: mode indicator + 16 bits.
                if reader.read(16).is_none() {
                    return Err(RejectReason::Truncated {
                        stage: Stage::Segments,
                    });
                }
            }
            5 | 9 => {
                // FNC1 first / second position — flag only; the '%'→GS
                // substitution applies to the alphanumeric output below.
                fnc1 = true;
            }
            _ => {
                return Err(RejectReason::ModeInvalid {
                    bit: mode_bit as u32,
                    mode,
                });
            }
        }
    }

    Ok((text, raw))
}

/// Byte-segment text interpretation, deterministic:
/// explicit UTF charsets first, otherwise UTF-8 when valid and
/// Latin-1 otherwise.
fn decode_bytes(
    bytes: &[u8],
    charset: Charset,
    utf8_default: bool,
    bit: usize,
) -> Result<String, RejectReason> {
    match charset {
        Charset::Utf16Be => {
            if !bytes.len().is_multiple_of(2) {
                return Err(RejectReason::DataUndecodable { bit: bit as u32 });
            }
            let units: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_be_bytes(*c))
                .collect();
            String::from_utf16(&units)
                .map_err(|_| RejectReason::DataUndecodable { bit: bit as u32 })
        }
        Charset::Utf8 => String::from_utf8(bytes.to_vec())
            .map_err(|_| RejectReason::DataUndecodable { bit: bit as u32 }),
        Charset::Latin1 => {
            if utf8_default && let Ok(s) = String::from_utf8(bytes.to_vec()) {
                return Ok(s);
            }
            Ok(bytes.iter().map(|&b| char::from(b)).collect())
        }
    }
}

/// Apply the FNC1 substitution: in the alphanumeric alphabet '%' is a
/// separator marker — '%%' collapses to '%', a lone '%' becomes GS
/// (U+001D / 0x1D). Text is transformed by characters and raw by raw
/// bytes independently; a '%' always occupies one char and one byte
/// (0x25), so the two layers never index into each other.
fn apply_fnc1(text: &mut String, raw: &mut Vec<u8>) {
    if !text.contains('%') {
        return;
    }
    let mut out = String::with_capacity(text.len());
    let mut iter = text.chars().peekable();
    while let Some(c) = iter.next() {
        if c == '%' {
            if iter.next_if_eq(&'%').is_some() {
                out.push('%');
            } else {
                out.push('\u{001D}');
            }
        } else {
            out.push(c);
        }
    }
    *text = out;

    let mut out_raw = Vec::with_capacity(raw.len());
    let mut i = 0usize;
    while i < raw.len() {
        if raw[i] == b'%' {
            if i + 1 < raw.len() && raw[i + 1] == b'%' {
                out_raw.push(b'%');
                i += 2;
            } else {
                out_raw.push(0x1D);
                i += 1;
            }
        } else {
            out_raw.push(raw[i]);
            i += 1;
        }
    }
    *raw = out_raw;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_reader_reads_msb_first() {
        let data = [0b1011_0011, 0x04];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read(4), Some(0b1011));
        assert_eq!(r.read(8), Some(0b0011_0000));
        assert_eq!(r.read(5), None);
        assert_eq!(r.read(4), Some(0b0100));
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn numeric_group_above_its_digit_count_is_rejected() {
        // Numeric, count = 3, then a 10-bit group holding 1000 — one more
        // than the 999 a 3-digit group may encode (ISO/IEC 18004 8.4.2).
        let data = [0b0001_0000, 0b0000_1111, 0b1110_1000];
        let err = parse_segments(&data, 1).unwrap_err();
        assert!(
            matches!(err, RejectReason::DataUndecodable { .. }),
            "expected DataUndecodable, got {err:?}"
        );
    }

    #[test]
    fn fnc1_marks_percent_in_each_layer_independently() {
        // Regression: raw slicing once used UTF-8 char lengths, so a
        // multi-byte char after '%' read past the end and panicked.
        // Text transforms by char, raw by byte — the layers never
        // index into each other.
        let mut text = String::from("A%B%C");
        let mut raw = vec![b'A', b'%', b'B', b'%', b'C'];
        apply_fnc1(&mut text, &mut raw);
        assert_eq!(text, "A\u{001D}B\u{001D}C");
        assert_eq!(raw, [b'A', 0x1D, b'B', 0x1D, b'C']);

        // '%%' collapses to a literal '%' in both layers.
        let mut text = String::from("A%%B");
        let mut raw = vec![b'A', b'%', b'%', b'B'];
        apply_fnc1(&mut text, &mut raw);
        assert_eq!(text, "A%B");
        assert_eq!(raw, [b'A', b'%', b'B']);
    }
}
