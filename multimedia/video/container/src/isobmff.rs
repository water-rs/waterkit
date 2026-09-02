//! Bounded top-level ISO Base Media File Format box access.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use waterkit_video_core::Error;

const BASIC_HEADER_LENGTH: usize = 8;
const BASIC_HEADER_SIZE: u64 = 8;
const EXTENDED_HEADER_SIZE: u64 = 16;

pub fn read_top_level_box(path: &Path, requested_type: [u8; 4]) -> Result<Option<Vec<u8>>, Error> {
    let mut file = std::fs::File::open(path)?;
    let file_len = file.metadata()?.len();
    let mut offset = 0_u64;

    while offset < file_len {
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
        let end = offset.checked_add(box_size).ok_or_else(|| {
            Error::Container(String::from("ISO BMFF top-level box range overflow"))
        })?;
        if end > file_len {
            return Err(Error::Container(format!(
                "ISO BMFF box {:?} ends at byte {end}, beyond the {file_len}-byte file",
                String::from_utf8_lossy(&box_type)
            )));
        }

        if box_type == requested_type {
            let allocation = usize::try_from(box_size).map_err(|_| {
                Error::Container(format!(
                    "ISO BMFF box {:?} exceeds the current architecture",
                    String::from_utf8_lossy(&box_type)
                ))
            })?;
            let mut bytes = vec![0_u8; allocation];
            file.seek(SeekFrom::Start(offset))?;
            file.read_exact(&mut bytes)?;
            return Ok(Some(bytes));
        }
        offset = end;
    }

    Ok(None)
}
