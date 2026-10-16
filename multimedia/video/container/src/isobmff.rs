//! Bounded top-level ISO Base Media File Format box access.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use waterkit_video_core::Error;

const BASIC_HEADER_LENGTH: usize = 8;
const BASIC_HEADER_SIZE: u64 = 8;
const EXTENDED_HEADER_SIZE: u64 = 16;

/// Location of one top-level ISO BMFF box inside a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TopLevelBoxSpan {
    /// The four-character box type.
    pub kind: [u8; 4],
    /// Absolute file offset of the box header.
    pub offset: u64,
    /// Absolute file offset of the box body.
    pub body_offset: u64,
    /// Total box size including its header.
    pub size: u64,
}

fn read_top_level_header(
    file: &mut std::fs::File,
    offset: u64,
    file_len: u64,
) -> Result<TopLevelBoxSpan, Error> {
    let remaining = file_len - offset;
    if remaining < BASIC_HEADER_SIZE {
        return Err(Error::Container(format!(
            "ISO BMFF file ends with {remaining} trailing bytes instead of a complete box header"
        )));
    }

    file.seek(SeekFrom::Start(offset))?;
    let mut basic_header = [0_u8; BASIC_HEADER_LENGTH];
    file.read_exact(&mut basic_header)?;
    let compact_size = u32::from_be_bytes([
        basic_header[0],
        basic_header[1],
        basic_header[2],
        basic_header[3],
    ]);
    let box_type = [
        basic_header[4],
        basic_header[5],
        basic_header[6],
        basic_header[7],
    ];

    let (box_size, header_size) = match compact_size {
        0 => (remaining, BASIC_HEADER_SIZE),
        1 => {
            if remaining < EXTENDED_HEADER_SIZE {
                return Err(Error::Container(format!(
                    "ISO BMFF box {:?} is missing its extended-size field",
                    String::from_utf8_lossy(&box_type)
                )));
            }
            let mut extended_size = [0_u8; 8];
            file.read_exact(&mut extended_size)?;
            (u64::from_be_bytes(extended_size), EXTENDED_HEADER_SIZE)
        }
        size => (u64::from(size), BASIC_HEADER_SIZE),
    };
    if box_size < header_size {
        return Err(Error::Container(format!(
            "ISO BMFF box {:?} declares size {box_size}, smaller than its {header_size}-byte header",
            String::from_utf8_lossy(&box_type)
        )));
    }
    let end = offset
        .checked_add(box_size)
        .ok_or_else(|| Error::Container(String::from("ISO BMFF top-level box range overflow")))?;
    if end > file_len {
        return Err(Error::Container(format!(
            "ISO BMFF box {:?} ends at byte {end}, beyond the {file_len}-byte file",
            String::from_utf8_lossy(&box_type)
        )));
    }

    Ok(TopLevelBoxSpan {
        kind: box_type,
        offset,
        body_offset: offset + header_size,
        size: box_size,
    })
}

/// Lists every top-level box in file order without reading box payloads.
pub fn scan_top_level_boxes(path: &Path) -> Result<Vec<TopLevelBoxSpan>, Error> {
    let mut file = std::fs::File::open(path)?;
    let file_len = file.metadata()?.len();
    let mut spans = Vec::new();
    let mut offset = 0_u64;
    while offset < file_len {
        let span = read_top_level_header(&mut file, offset, file_len)?;
        offset = span.offset + span.size;
        spans.push(span);
    }
    Ok(spans)
}

pub fn read_top_level_box(path: &Path, requested_type: [u8; 4]) -> Result<Option<Vec<u8>>, Error> {
    for span in scan_top_level_boxes(path)? {
        if span.kind != requested_type {
            continue;
        }
        let mut file = std::fs::File::open(path)?;
        let allocation = usize::try_from(span.size).map_err(|_| {
            Error::Container(format!(
                "ISO BMFF box {:?} exceeds the current architecture",
                String::from_utf8_lossy(&requested_type)
            ))
        })?;
        let mut bytes = vec![0_u8; allocation];
        file.seek(SeekFrom::Start(span.offset))?;
        file.read_exact(&mut bytes)?;
        return Ok(Some(bytes));
    }
    Ok(None)
}
