use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use roxmltree::Document;

use crate::{DialogError, LoadedLivePhoto};

const XMP_META_OPEN_TAGS: [&[u8]; 2] = [b"<x:xmpmeta", b"<xmpmeta"];
const XMP_META_CLOSE_TAGS: [&[u8]; 2] = [b"</x:xmpmeta>", b"</xmpmeta>"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MotionVideoType {
    Mp4,
    Mov,
}

impl MotionVideoType {
    const fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mov => "mov",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MotionPhotoMetadata {
    video_length: usize,
    video_type: MotionVideoType,
}

pub(crate) fn load_live_photo_from_motion_photo(
    path: &Path,
) -> Result<Option<LoadedLivePhoto>, DialogError> {
    let bytes = fs::read(path).map_err(|error| {
        DialogError::PlatformError(format!(
            "failed to read Android Motion Photo file '{}': {error}",
            path.display()
        ))
    })?;
    let Some(xmp) = extract_xmp_packet(&bytes)? else {
        return Ok(None);
    };
    let Some(metadata) = parse_motion_photo_metadata(xmp)? else {
        return Ok(None);
    };
    let video_path = write_motion_video(path, &bytes, metadata)?;
    Ok(Some(LoadedLivePhoto::new(path.to_path_buf(), video_path)))
}

fn extract_xmp_packet(bytes: &[u8]) -> Result<Option<&str>, DialogError> {
    let Some(start) = find_first_marker(bytes, &XMP_META_OPEN_TAGS) else {
        return Ok(None);
    };
    let (close_start, close_len) =
        find_first_marker_with_len(&bytes[start..], &XMP_META_CLOSE_TAGS).ok_or_else(|| {
            DialogError::PlatformError(
                "Android Motion Photo XMP packet is missing a closing tag".into(),
            )
        })?;
    let close_end = close_start + close_len;
    std::str::from_utf8(&bytes[start..start + close_end])
        .map(Some)
        .map_err(|error| {
            DialogError::PlatformError(format!(
                "Android Motion Photo XMP packet is not valid UTF-8: {error}"
            ))
        })
}

fn find_first_marker(haystack: &[u8], markers: &[&[u8]]) -> Option<usize> {
    find_first_marker_with_len(haystack, markers).map(|(position, _)| position)
}

fn find_first_marker_with_len(haystack: &[u8], markers: &[&[u8]]) -> Option<(usize, usize)> {
    markers
        .iter()
        .filter_map(|marker| {
            find_subslice(haystack, marker).map(|position| (position, marker.len()))
        })
        .min_by_key(|(position, _)| *position)
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn parse_motion_photo_metadata(xmp: &str) -> Result<Option<MotionPhotoMetadata>, DialogError> {
    let document = Document::parse(xmp).map_err(|error| {
        DialogError::PlatformError(format!("failed to parse Android Motion Photo XMP: {error}"))
    })?;
    if !motion_photo_flag(&document) {
        return Ok(None);
    }

    let mut motion_item = None;
    for node in document.descendants().filter(roxmltree::Node::is_element) {
        let mut semantic = None;
        let mut mime = None;
        let mut length = None;
        for attribute in node.attributes() {
            match attribute.name() {
                "Semantic" => semantic = Some(attribute.value()),
                "Mime" => mime = Some(attribute.value()),
                "Length" => length = Some(attribute.value()),
                _ => {}
            }
        }

        if semantic != Some("MotionPhoto") {
            continue;
        }

        let mime = mime.ok_or_else(|| {
            DialogError::PlatformError(
                "Android Motion Photo XMP motion item is missing its MIME type".into(),
            )
        })?;
        let length = length.ok_or_else(|| {
            DialogError::PlatformError(
                "Android Motion Photo XMP motion item is missing its length".into(),
            )
        })?;
        let metadata = MotionPhotoMetadata {
            video_length: parse_video_length(length)?,
            video_type: parse_motion_video_type(mime)?,
        };
        if motion_item.replace(metadata).is_some() {
            return Err(DialogError::PlatformError(
                "Android Motion Photo XMP contains multiple motion items".into(),
            ));
        }
    }

    motion_item.map(Some).ok_or_else(|| {
        DialogError::PlatformError(
            "Android Motion Photo XMP is missing container motion item metadata".into(),
        )
    })
}

fn motion_photo_flag(document: &Document<'_>) -> bool {
    document
        .descendants()
        .filter(roxmltree::Node::is_element)
        .flat_map(|node| node.attributes())
        .any(|attribute| {
            matches!(attribute.name(), "MotionPhoto" | "MicroVideo") && attribute.value() == "1"
        })
}

fn parse_video_length(value: &str) -> Result<usize, DialogError> {
    let length = value.parse::<usize>().map_err(|error| {
        DialogError::PlatformError(format!(
            "Android Motion Photo XMP length '{value}' is invalid: {error}"
        ))
    })?;
    if length == 0 {
        return Err(DialogError::PlatformError(
            "Android Motion Photo XMP length must be non-zero".into(),
        ));
    }
    Ok(length)
}

fn parse_motion_video_type(value: &str) -> Result<MotionVideoType, DialogError> {
    match value {
        "video/mp4" => Ok(MotionVideoType::Mp4),
        "video/quicktime" => Ok(MotionVideoType::Mov),
        _ => Err(DialogError::PlatformError(format!(
            "Android Motion Photo video MIME type '{value}' is unsupported"
        ))),
    }
}

fn write_motion_video(
    image_path: &Path,
    bytes: &[u8],
    metadata: MotionPhotoMetadata,
) -> Result<PathBuf, DialogError> {
    if metadata.video_length >= bytes.len() {
        return Err(DialogError::PlatformError(format!(
            "Android Motion Photo video length {} exceeds source file size {}",
            metadata.video_length,
            bytes.len()
        )));
    }
    let video_start = bytes.len() - metadata.video_length;
    let video_path = motion_video_path(image_path, metadata.video_type);
    fs::write(&video_path, &bytes[video_start..]).map_err(|error| {
        DialogError::PlatformError(format!(
            "failed to write extracted Android Motion Photo video '{}': {error}",
            video_path.display()
        ))
    })?;
    Ok(video_path)
}

fn motion_video_path(image_path: &Path, video_type: MotionVideoType) -> PathBuf {
    let file_name = image_path.file_name().unwrap_or_else(|| {
        panic!(
            "Android Motion Photo source path '{}' is missing a file name",
            image_path.display()
        )
    });
    let mut derived_name = OsString::from(file_name);
    derived_name.push(".motion.");
    derived_name.push(video_type.extension());
    image_path.with_file_name(derived_name)
}

#[cfg(test)]
mod tests {
    use super::{
        MotionVideoType, extract_xmp_packet, load_live_photo_from_motion_photo,
        parse_motion_photo_metadata,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn motion_photo_xmp(video_mime: &str, video_length: usize) -> String {
        format!(
            concat!(
                "<x:xmpmeta xmlns:x='adobe:ns:meta/' ",
                "xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#' ",
                "xmlns:Camera='http://ns.google.com/photos/1.0/camera/' ",
                "xmlns:Container='http://ns.google.com/photos/1.0/container/' ",
                "xmlns:Item='http://ns.google.com/photos/1.0/container/item/'>",
                "<rdf:RDF><rdf:Description Camera:MotionPhoto='1'>",
                "<Container:Directory><rdf:Seq>",
                "<rdf:li rdf:parseType='Resource' Item:Mime='image/jpeg' Item:Semantic='Primary' Item:Length='32'/>",
                "<rdf:li rdf:parseType='Resource' Item:Mime='{video_mime}' Item:Semantic='MotionPhoto' Item:Length='{video_length}'/>",
                "</rdf:Seq></Container:Directory>",
                "</rdf:Description></rdf:RDF></x:xmpmeta>"
            ),
            video_mime = video_mime,
            video_length = video_length
        )
    }

    fn unique_test_path(extension: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("waterkit-dialog-motion-photo-{stamp}.{extension}"))
    }

    #[test]
    fn extracts_xmp_packet_from_embedded_bytes() {
        let xmp = motion_photo_xmp("video/mp4", 16);
        let payload = [b"prefix".as_slice(), xmp.as_bytes(), b"suffix".as_slice()].concat();
        assert_eq!(extract_xmp_packet(&payload).unwrap(), Some(xmp.as_str()));
    }

    #[test]
    fn parses_container_motion_photo_metadata() {
        let xmp = motion_photo_xmp("video/quicktime", 24);
        let metadata = parse_motion_photo_metadata(&xmp).unwrap().unwrap();
        assert_eq!(metadata.video_length, 24);
        assert_eq!(metadata.video_type, MotionVideoType::Mov);
    }

    #[test]
    fn loads_motion_photo_into_paired_media_paths() {
        let video_bytes = [
            0_u8, 0, 0, 24, b'f', b't', b'y', b'p', b'i', b's', b'o', b'm', 0, 0, 0, 0, 1, 2, 3, 4,
            5, 6, 7, 8,
        ];
        let xmp = motion_photo_xmp("video/mp4", video_bytes.len());
        let image_bytes = [
            b"\xFF\xD8".as_slice(),
            xmp.as_bytes(),
            b"\xFF\xD9".as_slice(),
        ]
        .concat();
        let file_bytes = [image_bytes.as_slice(), video_bytes.as_slice()].concat();
        let image_path = unique_test_path("jpg");
        fs::write(&image_path, file_bytes).unwrap();

        let live_photo = load_live_photo_from_motion_photo(&image_path)
            .unwrap()
            .expect("motion photo should be detected");
        let expected_video_path = image_path.with_file_name(format!(
            "{}.motion.mp4",
            image_path.file_name().unwrap().to_string_lossy()
        ));
        assert_eq!(live_photo.image(), image_path.as_path());
        assert_eq!(live_photo.video(), expected_video_path.as_path());
        assert_eq!(fs::read(live_photo.video()).unwrap(), video_bytes);

        fs::remove_file(live_photo.video()).unwrap();
        fs::remove_file(image_path).unwrap();
    }

    #[test]
    fn ignores_regular_images_without_motion_photo_flag() {
        let xmp = concat!(
            "<x:xmpmeta xmlns:x='adobe:ns:meta/' xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>",
            "<rdf:RDF><rdf:Description /></rdf:RDF></x:xmpmeta>"
        );
        assert!(parse_motion_photo_metadata(xmp).unwrap().is_none());
    }
}
