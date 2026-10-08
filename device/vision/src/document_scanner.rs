use waterkit_core::Capabilities;

use crate::{Image, VisionError, sys};

/// The one-shot system document scanner.
///
/// Presents the platform's own document-scanning UI and resolves to the
/// scanned pages in order, each an [`Image`] a request like
/// [`RecognizeDocument`] or [`RecognizeText`] accepts directly, like the
/// workspace's dialog pickers. The caller never sees a camera frame:
///
/// - **Android:** the ML Kit document scanner of Google Play services
///   (`GmsDocumentScanning`), presenting Play services' own capture screen
///   that finds page edges, corrects perspective and cleans up each page.
///   It needs no camera permission from the app; the app only links the
///   thin `play-services-mlkit-document-scanner` client this crate declares
///   for the packager, and its scanning module is downloaded to the device
///   by Play services on first use. Pages come back as JPEG images; the
///   optional PDF result format is not requested.
/// - **iOS:** `VisionKit`'s `VNDocumentCameraViewController`, presented from
///   the app's key window scene. Its UI has neither a page limit nor a
///   gallery import, so configuring either is an
///   [`VisionError::Unsupported`] error rather than a silent no-op.
///
/// There is no system document scanner on macOS, Mac Catalyst, Windows,
/// Linux, wasm or Android devices without Play services:
/// [`DocumentScanner::capabilities`] reports the scanner unavailable there
/// and [`DocumentScanner::scan`] fails with [`VisionError::Unsupported`]. A
/// [`DocumentScanner`] is never silently served another way; `WaterUI` owns
/// the fallback capture view.
///
/// [`RecognizeDocument`]: crate::RecognizeDocument
/// [`RecognizeText`]: crate::RecognizeText
#[derive(Debug, Clone, Default)]
pub struct DocumentScanner {
    page_limit: Option<u16>,
    gallery_import: bool,
}

impl DocumentScanner {
    /// Creates a scanner with no page limit and gallery import off.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The page limit this scanner enforces, when one is configured.
    #[must_use]
    pub const fn page_limit(&self) -> Option<u16> {
        self.page_limit
    }

    /// Caps the scan at `page_limit` pages.
    ///
    /// Only the Android scanner enforces a page limit, as
    /// [`DocumentScannerCapabilities::page_limit`] reports; configuring one
    /// elsewhere makes [`DocumentScanner::scan`] fail with
    /// [`VisionError::Unsupported`].
    ///
    /// # Panics
    ///
    /// Panics when `page_limit` is zero: a scan limited to nothing can never
    /// return.
    #[must_use]
    pub const fn with_page_limit(mut self, page_limit: u16) -> Self {
        assert!(
            page_limit > 0,
            "a document scan's page limit must be at least one page"
        );
        self.page_limit = Some(page_limit);
        self
    }

    /// Whether this scanner may import pages from the gallery.
    #[must_use]
    pub const fn gallery_import(&self) -> bool {
        self.gallery_import
    }

    /// Allows the scan UI to import pages from the gallery.
    ///
    /// Only the Android scanner offers gallery import, as
    /// [`DocumentScannerCapabilities::gallery_import`] reports; enabling it
    /// elsewhere makes [`DocumentScanner::scan`] fail with
    /// [`VisionError::Unsupported`].
    #[must_use]
    pub const fn with_gallery_import(mut self, gallery_import: bool) -> Self {
        self.gallery_import = gallery_import;
        self
    }

    /// Probes whether this device can present the system document scanner.
    ///
    /// Reports the scanner available on Android only when Google Play
    /// services is usable on the device, and on iOS only when
    /// `VNDocumentCameraViewController` is supported; the simulator and all
    /// other platforms report unavailable. `page_limit` and
    /// `gallery_import` report which [`DocumentScanner`] options this
    /// platform's scanner honors — both only on Android. An unavailable
    /// scanner means [`DocumentScanner::scan`] fails: the fallback scanning
    /// UI lives in `WaterUI`, not here.
    ///
    /// # Panics
    ///
    /// On Android, panics if the application `Context` has not been
    /// published to `ndk_context` yet or the JNI probe fails.
    #[must_use]
    #[cfg_attr(
        not(any(
            target_os = "android",
            all(target_os = "ios", not(target_abi = "macabi"))
        )),
        expect(
            clippy::missing_const_for_fn,
            reason = "the iOS and Android availability probes are runtime calls; on other platforms the probe is a constant and clippy suggests const"
        )
    )]
    pub fn capabilities() -> DocumentScannerCapabilities {
        let options = sys::document_scanner_options();
        DocumentScannerCapabilities {
            available: sys::document_scanner_available(),
            page_limit: options.page_limit,
            gallery_import: options.gallery_import,
        }
    }

    /// Presents the system scanner and resolves to the scanned pages in
    /// order, or `Ok(None)` when the user cancels, like the dialog pickers.
    ///
    /// Every page is a decoded [`Image`]; passing a page to
    /// [`crate::Vision::perform`] with a [`crate::RecognizeDocument`] or
    /// [`crate::RecognizeText`] request needs no conversion step.
    ///
    /// # Errors
    ///
    /// Returns [`VisionError::Unsupported`] before presenting anything when
    /// the configured options include ones this platform's scanner cannot
    /// express — the error names them — and when this device has no system
    /// document scanner at all, the check
    /// [`DocumentScanner::capabilities`] performs. Returns
    /// [`VisionError::Platform`] when a supported scanner fails while
    /// presenting, scanning or delivering its pages.
    pub async fn scan(self) -> Result<Option<Vec<Image>>, VisionError> {
        let options = sys::document_scanner_options();
        let mut inexpressible = Vec::new();
        if self.page_limit.is_some() && !options.page_limit {
            inexpressible.push("page limit");
        }
        if self.gallery_import && !options.gallery_import {
            inexpressible.push("gallery import");
        }
        if !inexpressible.is_empty() {
            return Err(VisionError::Unsupported(format!(
                "this platform's document scanner cannot express: {}",
                inexpressible.join(", ")
            )));
        }
        sys::scan_document(self.page_limit, self.gallery_import).await
    }
}

/// Capability probe for the system document scanner, returned by
/// [`DocumentScanner::capabilities`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct DocumentScannerCapabilities {
    /// Whether the device can present the system document scanner.
    pub available: bool,
    /// Whether the platform's scanner enforces a page limit.
    ///
    /// Only Android's scanner does; elsewhere configuring
    /// [`DocumentScanner::with_page_limit`] fails the scan.
    pub page_limit: bool,
    /// Whether the platform's scanner can import pages from the gallery.
    ///
    /// Only Android's scanner can; elsewhere configuring
    /// [`DocumentScanner::with_gallery_import`] fails the scan.
    pub gallery_import: bool,
}

impl Capabilities for DocumentScannerCapabilities {
    fn available(&self) -> bool {
        self.available
    }
}

/// Which [`DocumentScanner`] options a platform's scanner honors, as
/// `sys::document_scanner_options` reports per platform.
#[derive(Debug, Clone, Copy)]
pub struct DocumentScannerOptions {
    pub page_limit: bool,
    pub gallery_import: bool,
}

/// Builds the pages a scan resolves to: one [`Image`] per encoded page, in
/// scan order. Both system scanners produce JPEG pages, which
/// [`Image::from_encoded`] feeds to every request with no conversion step.
#[cfg(any(
    target_os = "android",
    all(target_os = "ios", not(target_abi = "macabi")),
    all(test, any(target_os = "ios", target_os = "macos"))
))]
pub fn pages_from_encoded(pages: Vec<bytes::Bytes>) -> Vec<Image> {
    pages.into_iter().map(Image::from_encoded).collect()
}

/// Decodes the page payload the iOS bridge hands back: a JSON array of
/// base64-encoded JPEG pages, the format every bridge result in this crate
/// crosses in.
#[cfg(any(
    all(target_os = "ios", not(target_abi = "macabi")),
    all(test, any(target_os = "ios", target_os = "macos"))
))]
pub fn pages_from_base64_json(json: &str) -> Result<Vec<Image>, VisionError> {
    use base64::Engine as _;
    let encoded: Vec<String> = serde_json::from_str(json).map_err(|error| {
        VisionError::Platform(format!(
            "the document scanner returned an unreadable page payload: {error}"
        ))
    })?;
    let pages = encoded
        .iter()
        .map(|page| {
            base64::engine::general_purpose::STANDARD
                .decode(page)
                .map(bytes::Bytes::from)
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            VisionError::Platform(format!(
                "the document scanner returned an undecodable page: {error}"
            ))
        })?;
    Ok(pages_from_encoded(pages))
}

#[cfg(test)]
mod tests {
    use super::DocumentScanner;
    use crate::VisionError;

    #[test]
    #[cfg(not(any(
        target_os = "android",
        all(target_os = "ios", not(target_abi = "macabi"))
    )))]
    fn unsupported_platforms_report_unavailable_and_no_options() {
        let capabilities = DocumentScanner::capabilities();
        assert!(!capabilities.available);
        assert!(!capabilities.page_limit);
        assert!(!capabilities.gallery_import);
    }

    #[test]
    #[cfg(not(any(
        target_os = "android",
        all(target_os = "ios", not(target_abi = "macabi"))
    )))]
    fn scan_is_unsupported_where_no_system_scanner_exists() {
        let error = pollster::block_on(DocumentScanner::new().scan()).unwrap_err();
        assert!(matches!(error, VisionError::Unsupported(_)));
    }

    #[test]
    #[cfg(not(any(
        target_os = "android",
        all(target_os = "ios", not(target_abi = "macabi"))
    )))]
    fn scan_rejects_options_the_platform_cannot_express() {
        let error =
            pollster::block_on(DocumentScanner::new().with_page_limit(4).scan()).unwrap_err();
        assert!(
            matches!(error, VisionError::Unsupported(message) if message.contains("page limit"))
        );

        let error = pollster::block_on(
            DocumentScanner::new()
                .with_page_limit(4)
                .with_gallery_import(true)
                .scan(),
        )
        .unwrap_err();
        assert!(
            matches!(error, VisionError::Unsupported(message) if message.contains("page limit") && message.contains("gallery import"))
        );
    }

    #[test]
    #[should_panic(expected = "at least one page")]
    fn a_zero_page_limit_panics() {
        let _ = DocumentScanner::new().with_page_limit(0);
    }

    /// A page encoded at test time — no fixture is committed — must arrive
    /// in the scan result as an [`crate::image::Pixels::Encoded`] image a
    /// request accepts directly.
    #[test]
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    fn page_payloads_decode_to_encoded_images() {
        use base64::Engine as _;

        let mut rgba = image::RgbaImage::new(8, 8);
        for pixel in rgba.pixels_mut() {
            *pixel = image::Rgba([200, 180, 160, 255]);
        }
        let mut jpeg = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(rgba)
            .write_to(&mut jpeg, image::ImageFormat::Jpeg)
            .expect("the test page encodes as JPEG");
        let jpeg = jpeg.into_inner();

        let json = serde_json::to_string(&[
            base64::engine::general_purpose::STANDARD.encode(&jpeg),
            base64::engine::general_purpose::STANDARD.encode(&jpeg),
        ])
        .expect("the page payload serializes");

        let pages = super::pages_from_base64_json(&json).expect("the payload decodes");
        assert_eq!(pages.len(), 2);
        for page in &pages {
            let crate::image::Pixels::Encoded(bytes) = page.pixels() else {
                panic!("a scanned page keeps its encoded bytes")
            };
            let decoded = image::load_from_memory(bytes).expect("the page stays decodable");
            assert_eq!((decoded.width(), decoded.height()), (8, 8));
        }
    }
}