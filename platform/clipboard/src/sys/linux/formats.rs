//! The formats a selection is written and read in, shared by every display
//! server so that Wayland and X11 offer and accept the same ones.

use std::io::Cursor;
use std::path::PathBuf;

use image::{DynamicImage, ImageFormat};
use url::Url;

use crate::content::{ClipboardEvent, Image};
use crate::error::ClipboardError;
use crate::sys::file_path::not_absolute;

/// HTML markup.
pub const HTML: &str = "text/html";
/// A PNG image.
pub const PNG: &str = "image/png";
/// A list of URIs, one per line (RFC 2483).
pub const URI_LIST: &str = "text/uri-list";
/// Copied files as GNOME's and other file managers' paste reads them.
const GNOME_COPIED_FILES: &str = "x-special/gnome-copied-files";

/// The targets plain text is offered under and read from, in the order a
/// read prefers them. Each carries UTF-8: `UTF8_STRING` is X11's name for it,
/// the MIME types are Wayland's, and plain `text/plain` is UTF-8 in practice.
/// `STRING` (Latin-1) and `TEXT` (any encoding) are left out, because their
/// bytes are not UTF-8.
const TEXT: [&str; 3] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"];

/// One format of the content a write offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Representation {
    /// The MIME type, or on X11 the target name, it is offered under.
    pub mime: String,
    /// The bytes a read of that format receives.
    pub bytes: Vec<u8>,
}

impl Representation {
    /// `bytes` offered as `mime`.
    pub fn new(mime: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            mime: mime.into(),
            bytes: bytes.into(),
        }
    }
}

/// `text`, offered under every plain-text target.
pub fn text(text: &str) -> Vec<Representation> {
    TEXT.iter()
        .map(|&mime| Representation::new(mime, text))
        .collect()
}

/// `html`, and `alt_text` as plain text when given.
pub fn html(html: &str, alt_text: Option<&str>) -> Vec<Representation> {
    std::iter::once(Representation::new(HTML, html))
        .chain(alt_text.into_iter().flat_map(text))
        .collect()
}

/// `paths` as a URI list, as GNOME's copied files, and as plain text with one
/// path per line.
///
/// The plain text is for display, so a path that is not UTF-8 is shown with
/// replacement characters there; the two file formats carry every path
/// exactly.
///
/// # Errors
///
/// [`ClipboardError::Encode`] when a path is not absolute, which a file URI
/// requires.
pub fn files(paths: &[PathBuf]) -> Result<Vec<Representation>, ClipboardError> {
    let uris = paths
        .iter()
        .map(|path| Url::from_file_path(path).map_err(|()| not_absolute(path)))
        .collect::<Result<Vec<_>, _>>()?;
    let uri_list: String = uris.iter().flat_map(|uri| [uri.as_str(), "\r\n"]).collect();
    let copied_files = std::iter::once("copy")
        .chain(uris.iter().map(Url::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    let display = paths
        .iter()
        .map(|path| path.to_string_lossy())
        .collect::<Vec<_>>()
        .join("\n");
    Ok([
        Representation::new(URI_LIST, uri_list),
        Representation::new(GNOME_COPIED_FILES, copied_files),
    ]
    .into_iter()
    .chain(text(&display))
    .collect())
}

/// `image` encoded as PNG.
///
/// # Errors
///
/// [`ClipboardError::Encode`] when the PNG encoder rejects the image.
pub fn png(image: &DynamicImage) -> Result<Vec<Representation>, ClipboardError> {
    let mut png = Vec::new();
    DynamicImage::ImageRgba8(image.to_rgba8())
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .map_err(|error| ClipboardError::Encode(format!("failed to encode PNG: {error}")))?;
    Ok(vec![Representation::new(PNG, png)])
}

/// Text read from a plain-text target.
///
/// # Errors
///
/// [`ClipboardError::Decode`] when the bytes are not UTF-8.
pub fn decode_text(bytes: Vec<u8>) -> Result<String, ClipboardError> {
    String::from_utf8(bytes).map_err(|error| ClipboardError::Decode(error.to_string()))
}

/// The local files a URI list names.
///
/// Empty lines and `#` comments are skipped, and so are URIs of a scheme
/// other than `file`: they name no local file.
///
/// # Errors
///
/// [`ClipboardError::Decode`] when the list is not UTF-8, a line is not a
/// URI, or a file URI names a file on another host.
pub fn decode_uri_list(bytes: &[u8]) -> Result<Vec<PathBuf>, ClipboardError> {
    let list = std::str::from_utf8(bytes)
        .map_err(|error| ClipboardError::Decode(format!("{URI_LIST} is not UTF-8: {error}")))?;
    list.lines()
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| match Url::parse(line) {
            Ok(uri) if uri.scheme() == "file" => Some(uri.to_file_path().map_err(|()| {
                ClipboardError::Decode(format!("{line} names a file on another host"))
            })),
            Ok(_) => None,
            Err(error) => Some(Err(ClipboardError::Decode(format!(
                "{line:?} in {URI_LIST} is not a URI: {error}"
            )))),
        })
        .collect()
}

/// An image read from a PNG target, as RGBA pixels.
///
/// # Errors
///
/// [`ClipboardError::InvalidImage`] when the bytes are not a PNG image.
pub fn decode_png(bytes: &[u8]) -> Result<Image, ClipboardError> {
    let rgba = image::load_from_memory_with_format(bytes, ImageFormat::Png)
        .map_err(|error| ClipboardError::InvalidImage(error.to_string()))?
        .to_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(Image::new(width, height, rgba.into_raw()))
}

/// The formats a selection's owner offers.
#[derive(Debug, Clone, Default)]
pub struct Offered(Vec<String>);

impl Offered {
    /// The formats in `mime_types`.
    pub const fn new(mime_types: Vec<String>) -> Self {
        Self(mime_types)
    }

    /// Whether `mime` is offered.
    pub fn has(&self, mime: &str) -> bool {
        self.0.iter().any(|offered| offered == mime)
    }

    /// The offered plain-text target a text read uses.
    pub fn text_target(&self) -> Option<&'static str> {
        TEXT.into_iter().find(|&target| self.has(target))
    }

    /// Whether plain text is offered.
    pub fn has_text(&self) -> bool {
        self.text_target().is_some()
    }

    /// Whether HTML is offered.
    pub fn has_html(&self) -> bool {
        self.has(HTML)
    }

    /// Whether files are offered.
    pub fn has_files(&self) -> bool {
        self.has(URI_LIST)
    }

    /// Whether an image is offered.
    pub fn has_image(&self) -> bool {
        self.has(PNG)
    }

    /// The change event announcing these formats.
    pub fn event(&self) -> ClipboardEvent {
        ClipboardEvent::new(
            self.has_text(),
            self.has_html(),
            self.has_files(),
            self.has_image(),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use image::{DynamicImage, RgbaImage};

    use super::{
        HTML, Offered, PNG, Representation, URI_LIST, decode_png, decode_uri_list, files, html, png,
    };
    use crate::error::ClipboardError;

    fn offered(representations: &[Representation]) -> Offered {
        Offered::new(
            representations
                .iter()
                .map(|representation| representation.mime.clone())
                .collect(),
        )
    }

    fn bytes_of<'a>(representations: &'a [Representation], mime: &str) -> &'a [u8] {
        &representations
            .iter()
            .find(|representation| representation.mime == mime)
            .unwrap_or_else(|| panic!("{mime} is not offered"))
            .bytes
    }

    #[test]
    fn html_offers_alt_text_as_plain_text_only_when_given() {
        let with_alt = html("<b>bold</b>", Some("bold"));
        assert_eq!(bytes_of(&with_alt, HTML), b"<b>bold</b>");
        let offered_with_alt = offered(&with_alt);
        let target = offered_with_alt.text_target().expect("alt text is offered");
        assert_eq!(bytes_of(&with_alt, target), b"bold");

        let without_alt = offered(&html("<b>bold</b>", None));
        assert!(without_alt.has_html());
        assert!(!without_alt.has_text());
    }

    #[test]
    fn text_read_prefers_the_charset_utf8_mime_type() {
        let offered = Offered::new(vec![
            "text/plain".into(),
            "UTF8_STRING".into(),
            "text/plain;charset=utf-8".into(),
        ]);
        assert_eq!(offered.text_target(), Some("text/plain;charset=utf-8"));
        let latin1_only = Offered::new(vec!["STRING".into(), "TEXT".into(), HTML.into()]);
        assert_eq!(latin1_only.text_target(), None);
    }

    #[test]
    fn files_round_trip_through_the_uri_list() {
        let paths = vec![
            PathBuf::from("/tmp/a file.txt"),
            PathBuf::from("/tmp/n\u{e4}me#1?.txt"),
        ];
        let representations = files(&paths).unwrap();
        let uri_list = bytes_of(&representations, URI_LIST);
        assert_eq!(
            uri_list,
            b"file:///tmp/a%20file.txt\r\nfile:///tmp/n%C3%A4me%231%3F.txt\r\n"
        );
        assert_eq!(decode_uri_list(uri_list).unwrap(), paths);
        assert_eq!(
            bytes_of(&representations, "x-special/gnome-copied-files"),
            b"copy\nfile:///tmp/a%20file.txt\nfile:///tmp/n%C3%A4me%231%3F.txt"
        );
        let offered = offered(&representations);
        assert!(offered.has_files());
        let target = offered.text_target().expect("paths are offered as text");
        assert_eq!(
            bytes_of(&representations, target),
            "/tmp/a file.txt\n/tmp/n\u{e4}me#1?.txt".as_bytes()
        );
    }

    #[test]
    fn files_rejects_a_relative_path() {
        assert!(matches!(
            files(&[PathBuf::from("relative.txt")]),
            Err(ClipboardError::Encode(_))
        ));
    }

    #[test]
    fn uri_list_skips_comments_and_other_schemes() {
        let list = b"# comment\nhttps://example.com/\n\nfile:///tmp/x\n";
        assert_eq!(
            decode_uri_list(list).unwrap(),
            vec![PathBuf::from("/tmp/x")]
        );
    }

    #[test]
    fn uri_list_rejects_remote_files_and_non_uris() {
        for list in [&b"file://elsewhere/tmp/x\r\n"[..], b"not a uri\r\n"] {
            assert!(matches!(
                decode_uri_list(list),
                Err(ClipboardError::Decode(_))
            ));
        }
    }

    #[test]
    fn png_round_trips_rgba_pixels() {
        let pixels = vec![255, 0, 0, 255, 0, 0, 255, 128];
        let image = DynamicImage::ImageRgba8(RgbaImage::from_raw(2, 1, pixels.clone()).unwrap());
        let representations = png(&image).unwrap();
        assert!(offered(&representations).has_image());
        let decoded = decode_png(bytes_of(&representations, PNG)).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (2, 1));
        assert_eq!(decoded.bytes(), pixels);
    }

    #[test]
    fn png_decode_rejects_other_bytes() {
        assert!(matches!(
            decode_png(b"not a png"),
            Err(ClipboardError::InvalidImage(_))
        ));
    }
}
