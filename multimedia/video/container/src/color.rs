use std::{
    fs::File,
    io::{Cursor, Read, Seek, SeekFrom},
    path::Path,
};

use waterkit_video_core::{
    ColorPrimaries, ColorRange, ContentLightLevel, Error, MatrixCoefficients, TransferFunction,
    VideoColorInfo,
};

#[derive(Debug, Clone, Copy)]
struct NclxColorInfo {
    primaries: u16,
    transfer: u16,
    matrix: u16,
    full_range: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct ColorMetadata {
    nclx: Option<NclxColorInfo>,
    content_light_level: Option<ContentLightLevel>,
    dolby_vision: bool,
}

#[derive(Debug, Clone, Copy)]
struct BoxHeader {
    kind: [u8; 4],
    content_start: u64,
    end: u64,
}

/// Reads color, HDR, and Dolby Vision signaling from an MP4/MOV file.
///
/// The parser seeks over media samples and large sample tables. It reads only
/// box headers and the small color-configuration payloads nested below `moov`,
/// so probing cost is independent of encoded media-data size. A file without
/// explicit signaling receives the conventional SD/HD default selected from
/// `height_hint`.
///
/// # Errors
///
/// Returns an I/O or container error when the MP4 box hierarchy is malformed
/// or cannot be traversed safely.
pub fn probe_mp4_color_info(
    path: impl AsRef<Path>,
    height_hint: Option<u32>,
) -> Result<VideoColorInfo, Error> {
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();
    probe_mp4_color_info_reader(&mut file, file_len, height_hint)
}

/// Reads color, HDR, and Dolby Vision signaling from in-memory MP4 initialization bytes.
///
/// # Errors
///
/// Returns a container error when the MP4 box hierarchy is malformed.
pub fn probe_mp4_color_info_bytes(
    bytes: &[u8],
    height_hint: Option<u32>,
) -> Result<VideoColorInfo, Error> {
    let file_len = u64::try_from(bytes.len())
        .map_err(|_| Error::Container(String::from("MP4 byte length exceeds u64")))?;
    probe_mp4_color_info_reader(&mut Cursor::new(bytes), file_len, height_hint)
}

fn probe_mp4_color_info_reader(
    file: &mut (impl Read + Seek),
    file_len: u64,
    height_hint: Option<u32>,
) -> Result<VideoColorInfo, Error> {
    let mut metadata = ColorMetadata::default();

    visit_boxes(file, 0, file_len, &mut metadata, VisitContext::TopLevel)?;
    Ok(resolve_color_info(metadata, height_hint))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisitContext {
    TopLevel,
    Container,
    SampleDescription,
    VisualSampleEntry,
}

fn visit_boxes(
    file: &mut (impl Read + Seek),
    start: u64,
    end: u64,
    metadata: &mut ColorMetadata,
    context: VisitContext,
) -> Result<(), Error> {
    let mut offset = start;
    while offset < end {
        let Some(header) = read_box_header(file, offset, end)? else {
            return Err(Error::Container(format!(
                "MP4 box range has {} trailing bytes, fewer than a box header",
                end.saturating_sub(offset)
            )));
        };

        match (context, header.kind) {
            (VisitContext::TopLevel, kind) if kind == *b"moov" => {
                visit_boxes(
                    file,
                    header.content_start,
                    header.end,
                    metadata,
                    VisitContext::Container,
                )?;
                return Ok(());
            }
            (VisitContext::Container, kind) if is_regular_container(kind) => {
                visit_boxes(
                    file,
                    header.content_start,
                    header.end,
                    metadata,
                    VisitContext::Container,
                )?;
            }
            (VisitContext::Container, kind) if kind == *b"stsd" => {
                let entries_start = checked_payload_start(&header, 8, "stsd")?;
                visit_boxes(
                    file,
                    entries_start,
                    header.end,
                    metadata,
                    VisitContext::SampleDescription,
                )?;
            }
            (VisitContext::SampleDescription, kind) if is_visual_sample_entry(kind) => {
                let children_start = checked_payload_start(&header, 78, "visual sample entry")?;
                visit_boxes(
                    file,
                    children_start,
                    header.end,
                    metadata,
                    VisitContext::VisualSampleEntry,
                )?;
            }
            (VisitContext::VisualSampleEntry, kind) if kind == *b"colr" => {
                if let Some(nclx) = read_nclx(file, &header)? {
                    metadata.nclx = Some(nclx);
                }
            }
            (VisitContext::VisualSampleEntry, kind) if kind == *b"dvcC" || kind == *b"dvvC" => {
                metadata.dolby_vision |= read_dolby_vision_configuration(file, &header)?;
            }
            (VisitContext::VisualSampleEntry, kind) if kind == *b"clli" => {
                metadata.content_light_level = Some(read_content_light_level(file, &header)?);
            }
            (VisitContext::VisualSampleEntry, kind) if kind == *b"sinf" => {
                visit_boxes(
                    file,
                    header.content_start,
                    header.end,
                    metadata,
                    VisitContext::Container,
                )?;
            }
            _ => {}
        }

        offset = header.end;
    }
    Ok(())
}

const fn is_regular_container(kind: [u8; 4]) -> bool {
    matches!(
        kind,
        [b't', b'r', b'a', b'k']
            | [b'm', b'd', b'i', b'a']
            | [b'm' | b's', b'i', b'n', b'f']
            | [b's', b't', b'b', b'l']
            | [b's', b'c', b'h', b'i']
    )
}

const fn is_visual_sample_entry(kind: [u8; 4]) -> bool {
    matches!(
        kind,
        [b'a' | b'h', b'v', b'c', b'1']
            | [b'a', b'v', b'c', b'3']
            | [b'h', b'e', b'v', b'1']
            | [b'd', b'v', b'h', b'e' | b'1']
            | [b'e', b'n', b'c', b'v']
    )
}

fn checked_payload_start(header: &BoxHeader, skip: u64, description: &str) -> Result<u64, Error> {
    let start = header
        .content_start
        .checked_add(skip)
        .ok_or_else(|| Error::Container(format!("{description} child-box offset overflow")))?;
    if start > header.end {
        return Err(Error::Container(format!(
            "{description} is smaller than its required {skip}-byte prelude"
        )));
    }
    Ok(start)
}

fn read_box_header(
    file: &mut (impl Read + Seek),
    offset: u64,
    parent_end: u64,
) -> Result<Option<BoxHeader>, Error> {
    if parent_end.saturating_sub(offset) < 8 {
        return Ok(None);
    }

    file.seek(SeekFrom::Start(offset))?;
    let size32 = read_u32(file)?;
    let mut kind = [0u8; 4];
    file.read_exact(&mut kind)?;
    let (box_size, header_size) = match size32 {
        0 => (parent_end.saturating_sub(offset), 8),
        1 => (read_u64(file)?, 16),
        size => (u64::from(size), 8),
    };
    if box_size < header_size {
        return Err(Error::Container(format!(
            "MP4 box {} has size {box_size}, smaller than its {header_size}-byte header",
            String::from_utf8_lossy(&kind)
        )));
    }
    let end = offset
        .checked_add(box_size)
        .ok_or_else(|| Error::Container(String::from("MP4 box size overflow")))?;
    if end > parent_end {
        return Err(Error::Container(format!(
            "MP4 box {} ends at byte {end}, beyond parent end {parent_end}",
            String::from_utf8_lossy(&kind)
        )));
    }

    Ok(Some(BoxHeader {
        kind,
        content_start: offset + header_size,
        end,
    }))
}

fn read_nclx(
    file: &mut (impl Read + Seek),
    header: &BoxHeader,
) -> Result<Option<NclxColorInfo>, Error> {
    let payload_len = header.end.saturating_sub(header.content_start);
    if payload_len < 10 {
        return Err(Error::Container(String::from(
            "MP4 colr box is smaller than its color-type and color fields",
        )));
    }
    file.seek(SeekFrom::Start(header.content_start))?;
    let mut color_type = [0u8; 4];
    file.read_exact(&mut color_type)?;
    if color_type != *b"nclx" && color_type != *b"nclc" {
        return Ok(None);
    }

    let primaries = read_u16(file)?;
    let transfer = read_u16(file)?;
    let matrix = read_u16(file)?;
    let full_range = if color_type == *b"nclx" {
        if payload_len < 11 {
            return Err(Error::Container(String::from(
                "MP4 nclx color box has no full-range flag",
            )));
        }
        let mut range = [0u8; 1];
        file.read_exact(&mut range)?;
        if range[0] & 0x7f != 0 {
            return Err(Error::Container(String::from(
                "MP4 nclx full-range byte has non-zero reserved bits",
            )));
        }
        range[0] & 0x80 != 0
    } else {
        false
    };

    Ok(Some(NclxColorInfo {
        primaries,
        transfer,
        matrix,
        full_range,
    }))
}

fn read_dolby_vision_configuration(
    file: &mut (impl Read + Seek),
    header: &BoxHeader,
) -> Result<bool, Error> {
    if header.end.saturating_sub(header.content_start) < 4 {
        return Err(Error::Container(String::from(
            "Dolby Vision configuration box is smaller than four bytes",
        )));
    }
    file.seek(SeekFrom::Start(header.content_start))?;
    let mut config = [0u8; 4];
    file.read_exact(&mut config)?;
    let profile = (config[2] & 0xfe) >> 1;
    let level = ((config[2] & 0x01) << 5) | ((config[3] & 0xf8) >> 3);
    Ok(profile > 0 && level > 0)
}

fn read_content_light_level(
    file: &mut (impl Read + Seek),
    header: &BoxHeader,
) -> Result<ContentLightLevel, Error> {
    if header.end.saturating_sub(header.content_start) < 4 {
        return Err(Error::Container(String::from(
            "content-light-level box is smaller than four bytes",
        )));
    }
    file.seek(SeekFrom::Start(header.content_start))?;
    Ok(ContentLightLevel::new(read_u16(file)?, read_u16(file)?))
}

fn resolve_color_info(metadata: ColorMetadata, height_hint: Option<u32>) -> VideoColorInfo {
    let mut info = metadata.nclx.map_or_else(
        || inferred_sdr_color_info(height_hint),
        |nclx| VideoColorInfo {
            matrix: map_matrix(nclx.matrix, height_hint),
            primaries: map_primaries(nclx.primaries, height_hint),
            transfer: map_transfer(nclx.transfer),
            range: if nclx.full_range {
                ColorRange::Full
            } else {
                ColorRange::Limited
            },
            content_light_level: metadata.content_light_level,
            dolby_vision: false,
        },
    );
    info.content_light_level = metadata.content_light_level;
    if metadata.dolby_vision {
        info.dolby_vision = true;
        if info.transfer == TransferFunction::Sdr {
            info.transfer = TransferFunction::Pq;
        }
        if info.primaries == ColorPrimaries::Bt709 {
            info.primaries = ColorPrimaries::Bt2020;
        }
    }
    info
}

pub fn inferred_sdr_color_info(height_hint: Option<u32>) -> VideoColorInfo {
    let standard_definition = height_hint.is_some_and(|height| height <= 576);
    VideoColorInfo {
        matrix: if standard_definition {
            MatrixCoefficients::Bt601
        } else {
            MatrixCoefficients::Bt709
        },
        primaries: if standard_definition {
            ColorPrimaries::Bt601
        } else {
            ColorPrimaries::Bt709
        },
        ..VideoColorInfo::default()
    }
}

fn map_matrix(matrix: u16, height_hint: Option<u32>) -> MatrixCoefficients {
    match matrix {
        1 => MatrixCoefficients::Bt709,
        5 | 6 => MatrixCoefficients::Bt601,
        9 => MatrixCoefficients::Bt2020NonConstantLuminance,
        10 => MatrixCoefficients::Bt2020ConstantLuminance,
        _ => inferred_sdr_color_info(height_hint).matrix,
    }
}

fn map_primaries(primaries: u16, height_hint: Option<u32>) -> ColorPrimaries {
    match primaries {
        1 => ColorPrimaries::Bt709,
        5..=7 => ColorPrimaries::Bt601,
        9..=10 => ColorPrimaries::Bt2020,
        11..=12 => ColorPrimaries::DisplayP3,
        _ => inferred_sdr_color_info(height_hint).primaries,
    }
}

const fn map_transfer(transfer: u16) -> TransferFunction {
    match transfer {
        16 => TransferFunction::Pq,
        18 => TransferFunction::Hlg,
        _ => TransferFunction::Sdr,
    }
}

fn read_u16(reader: &mut impl Read) -> Result<u16, Error> {
    let mut bytes = [0u8; 2];
    reader.read_exact(&mut bytes)?;
    Ok(u16::from_be_bytes(bytes))
}

fn read_u32(reader: &mut impl Read) -> Result<u32, Error> {
    let mut bytes = [0u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_be_bytes(bytes))
}

fn read_u64(reader: &mut impl Read) -> Result<u64, Error> {
    let mut bytes = [0u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_be_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use waterkit_video_core::{
        ColorPrimaries, ColorRange, ContentLightLevel, MatrixCoefficients, TransferFunction,
    };

    use super::{ColorMetadata, NclxColorInfo, resolve_color_info};

    #[test]
    fn nclx_metadata_retains_hdr_color_signals() {
        let info = resolve_color_info(
            ColorMetadata {
                nclx: Some(NclxColorInfo {
                    primaries: 9,
                    transfer: 16,
                    matrix: 9,
                    full_range: true,
                }),
                content_light_level: Some(ContentLightLevel::new(1_000, 400)),
                dolby_vision: false,
            },
            Some(2_160),
        );
        assert_eq!(info.matrix, MatrixCoefficients::Bt2020NonConstantLuminance);
        assert_eq!(info.primaries, ColorPrimaries::Bt2020);
        assert_eq!(info.transfer, TransferFunction::Pq);
        assert_eq!(info.range, ColorRange::Full);
        assert_eq!(
            info.content_light_level,
            Some(ContentLightLevel::new(1_000, 400))
        );
        assert!(info.is_hdr());
    }

    #[test]
    fn dolby_vision_promotes_unsignaled_color_to_hdr_bt2020() {
        let info = resolve_color_info(
            ColorMetadata {
                dolby_vision: true,
                ..ColorMetadata::default()
            },
            Some(2_160),
        );
        assert!(info.dolby_vision);
        assert_eq!(info.transfer, TransferFunction::Pq);
        assert_eq!(info.primaries, ColorPrimaries::Bt2020);
    }

    #[test]
    fn absent_signaling_uses_sd_matrix_only_for_sd_height() {
        let sd = resolve_color_info(ColorMetadata::default(), Some(576));
        let hd = resolve_color_info(ColorMetadata::default(), Some(1_080));
        assert_eq!(sd.matrix, MatrixCoefficients::Bt601);
        assert_eq!(hd.matrix, MatrixCoefficients::Bt709);
    }
}
