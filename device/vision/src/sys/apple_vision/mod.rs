//! The Apple native realization through the system Vision framework.
//!
//! Each pass prepares one `ImageRequestHandler` from the image's own
//! representation — a camera frame's `CVPixelBuffer`, a texture's
//! `MTLTexture`, or encoded bytes — and every request in the pass shares it.
//! Parameters and results cross the bridge as JSON, as the crate's other
//! Apple bridges do.

#[cfg(any(feature = "barcode", feature = "text", feature = "document"))]
use futures::channel::oneshot;
#[cfg(feature = "barcode")]
use std::sync::OnceLock;
#[cfg(any(feature = "barcode", feature = "text", feature = "document"))]
use std::{ffi::c_void, ptr};

#[cfg(feature = "barcode")]
use crate::sealed::{Offer, Pass};
#[cfg(any(feature = "barcode", feature = "text", feature = "document"))]
use crate::{
    VisionError,
    image::Pixels,
    sealed::{Context, Preparation},
};

#[swift_bridge::bridge]
pub mod ffi {
    extern "Swift" {
        // A JSON array of canonical symbology names Vision serves on this OS;
        // empty below the request API's availability.
        fn vision_supported_symbologies() -> String;
        // Retains a request handler for a retained `CVPixelBuffer` and its
        // EXIF `orientation`; 0 when Vision cannot take the image.
        fn vision_handler_pixel_buffer(buffer: usize, orientation: u8) -> usize;
        // Same for a retained `MTLTexture`; 0 when the texture is not
        // shader-readable.
        fn vision_handler_metal_texture(texture: usize, orientation: u8) -> usize;
        // Same for encoded image bytes; `orientation` 0 reads the embedded
        // EXIF metadata instead.
        fn vision_handler_data(data: Vec<u8>, orientation: u8) -> usize;
        // Releases a retained request handler.
        fn vision_handler_release(handler: usize);
        // Runs barcode detection; `symbologies` is a JSON array of canonical
        // names and the callback receives the JSON outcome.
        fn vision_detect_barcodes(
            handler: usize,
            symbologies: &str,
            callback: Box<dyn FnOnce(String)>,
        );
        // A JSON array of BCP-47 languages `recognitionLevel` `level`
        // (0 = fast) serves; empty below the request API's availability.
        pub fn vision_supported_text_languages(level: u8) -> String;
        // Runs text recognition at `recognitionLevel` `level` (0 = fast)
        // constrained to the JSON `languages` array; the callback receives
        // the JSON outcome.
        pub fn vision_recognize_text(
            handler: usize,
            level: u8,
            languages: &str,
            callback: Box<dyn FnOnce(String)>,
        );
        // A JSON array of BCP-47 languages `RecognizeDocumentsRequest`
        // serves; empty below the request API's availability.
        pub fn vision_supported_document_languages() -> String;
        // Runs document recognition constrained to the JSON `languages`
        // array; the callback receives the JSON outcome.
        pub fn vision_recognize_document(
            handler: usize,
            languages: &str,
            callback: Box<dyn FnOnce(String)>,
        );
    }
}

/// The request handler shared by requests in one pass.
#[cfg(any(feature = "barcode", feature = "text", feature = "document"))]
#[derive(Debug)]
pub struct AppleImage {
    /// The retained `ImageRequestHandler`, as an `Unmanaged` pointer.
    pub handler: usize,
}

#[cfg(any(feature = "barcode", feature = "text", feature = "document"))]
impl Drop for AppleImage {
    fn drop(&mut self) {
        ffi::vision_handler_release(self.handler);
    }
}

#[cfg(any(feature = "barcode", feature = "text", feature = "document"))]
impl Preparation for AppleImage {
    async fn prepare(_context: Context<'_>, pixels: &Pixels) -> Result<Self, VisionError> {
        let handler = match pixels {
            Pixels::Texture {
                texture,
                orientation,
            } => ffi::vision_handler_metal_texture(metal_texture(texture)?, orientation.exif()),
            #[cfg(feature = "camera")]
            Pixels::Frame {
                planes,
                orientation,
                pixel_buffer,
                ..
            } => match pixel_buffer {
                Some(buffer) => {
                    // The buffer stays retained for the FFI call's duration;
                    // the handler holds its own reference on the Swift side.
                    let buffer =
                        objc2_core_foundation::CFRetained::as_ptr(&buffer.0).as_ptr() as usize;
                    ffi::vision_handler_pixel_buffer(buffer, orientation.exif())
                }
                None => match planes {
                    waterkit_camera::FramePlanes::Rgb(view) => ffi::vision_handler_metal_texture(
                        metal_texture(view.texture())?,
                        orientation.exif(),
                    ),
                    waterkit_camera::FramePlanes::YCbCr420 { .. }
                    | waterkit_camera::FramePlanes::YCbCr422 { .. } => {
                        return Err(VisionError::Platform(
                            "a camera frame without its captured CVPixelBuffer cannot be served from YCbCr planes alone"
                                .to_owned(),
                        ));
                    }
                },
            },
            #[cfg(feature = "camera")]
            Pixels::Analysis { frame } => {
                // The frame's capture buffer serves Vision directly; the
                // handler holds its own reference on the Swift side.
                let buffer = frame.pixel_buffer();
                ffi::vision_handler_pixel_buffer(
                    objc2_core_foundation::CFRetained::as_ptr(&buffer).as_ptr() as usize,
                    frame.orientation().exif(),
                )
            }
            Pixels::Encoded(bytes) => ffi::vision_handler_data(Vec::from(bytes.as_ref()), 0),
        };
        if handler == 0 {
            return Err(VisionError::Platform(
                "Apple Vision could not build a request handler for this image".to_owned(),
            ));
        }
        Ok(Self { handler })
    }
}

/// The retained `MTLTexture` a wgpu texture is, as an FFI pointer.
#[cfg(any(feature = "barcode", feature = "text", feature = "document"))]
fn metal_texture(texture: &wgpu::Texture) -> Result<usize, VisionError> {
    // SAFETY: `texture` outlives the FFI call this handle is passed to; the
    // hal texture is only read for its raw handle.
    let hal = unsafe { texture.as_hal::<wgpu_hal::api::Metal>() }.ok_or_else(|| {
        VisionError::Platform("vision image textures must live on the Metal backend".to_owned())
    })?;
    Ok(ptr::from_ref(hal.raw_handle()).cast::<c_void>() as usize)
}

/// Runs `call` with a result callback and awaits the JSON it answers.
#[cfg(any(feature = "barcode", feature = "text", feature = "document"))]
pub async fn ffi_outcome<T: serde::de::DeserializeOwned>(
    call: impl FnOnce(Box<dyn FnOnce(String)>),
) -> Result<Vec<T>, VisionError> {
    let (sender, receiver) = oneshot::channel();
    call(Box::new(move |json| {
        // The receiver is gone only when the caller stopped waiting; the
        // answer then has no reader.
        let _ = sender.send(json);
    }));
    let json = receiver.await.map_err(|_| {
        VisionError::Platform("Apple Vision dropped the result callback".to_owned())
    })?;
    let outcome: WireOutcome<T> = serde_json::from_str(&json)
        .map_err(|error| VisionError::Platform(format!("malformed Vision result: {error}")))?;
    match outcome.error {
        Some(error) => Err(VisionError::Platform(error)),
        None => Ok(outcome.results.unwrap_or_default()),
    }
}

/// The JSON envelope every Vision result callback returns.
#[cfg(any(feature = "barcode", feature = "text", feature = "document"))]
#[derive(Debug, serde::Deserialize)]
pub struct WireOutcome<T> {
    /// The results; absent on failure.
    results: Option<Vec<T>>,
    /// The failure description.
    error: Option<String>,
}

/// Corners normalized to the upright image, decoded from interleaved x/y.
#[cfg(any(feature = "barcode", feature = "text", feature = "document"))]
pub const fn wire_quad(corners: [f32; 8]) -> crate::Quad {
    crate::Quad([
        crate::Point {
            x: corners[0],
            y: corners[1],
        },
        crate::Point {
            x: corners[2],
            y: corners[3],
        },
        crate::Point {
            x: corners[4],
            y: corners[5],
        },
        crate::Point {
            x: corners[6],
            y: corners[7],
        },
    ])
}

/// The canonical name the bridge passes a `symbology` in.
///
/// The vocabulary's own types stay shared with the scanner branch; naming
/// the constants is this realization's own mapping.
#[cfg(feature = "barcode")]
const fn name(symbology: crate::Symbology) -> &'static str {
    use crate::Symbology;
    match symbology {
        Symbology::Aztec => "aztec",
        Symbology::Codabar => "codabar",
        Symbology::Code39 => "code-39",
        Symbology::Code93 => "code-93",
        Symbology::Code128 => "code-128",
        Symbology::DataMatrix => "data-matrix",
        Symbology::Ean8 => "ean-8",
        Symbology::Ean13 => "ean-13",
        Symbology::Gs1DataBar => "gs1-databar",
        Symbology::Gs1DataBarExpanded => "gs1-databar-expanded",
        Symbology::Gs1DataBarLimited => "gs1-databar-limited",
        Symbology::Itf => "itf",
        Symbology::Itf14 => "itf-14",
        Symbology::MicroPdf417 => "micro-pdf-417",
        Symbology::MicroQr => "micro-qr",
        Symbology::MsiPlessey => "msi-plessey",
        Symbology::Pdf417 => "pdf-417",
        Symbology::Qr => "qr",
        Symbology::UpcA => "upc-a",
        Symbology::UpcE => "upc-e",
    }
}

/// The symbology a canonical `name` is.
#[cfg(any(feature = "barcode", feature = "document"))]
fn symbology_from_name(name: &str) -> Option<crate::Symbology> {
    use crate::Symbology;
    Some(match name {
        "aztec" => Symbology::Aztec,
        "codabar" => Symbology::Codabar,
        "code-39" => Symbology::Code39,
        "code-93" => Symbology::Code93,
        "code-128" => Symbology::Code128,
        "data-matrix" => Symbology::DataMatrix,
        "ean-8" => Symbology::Ean8,
        "ean-13" => Symbology::Ean13,
        "gs1-databar" => Symbology::Gs1DataBar,
        "gs1-databar-expanded" => Symbology::Gs1DataBarExpanded,
        "gs1-databar-limited" => Symbology::Gs1DataBarLimited,
        "itf" => Symbology::Itf,
        "itf-14" => Symbology::Itf14,
        "micro-pdf-417" => Symbology::MicroPdf417,
        "micro-qr" => Symbology::MicroQr,
        "msi-plessey" => Symbology::MsiPlessey,
        "pdf-417" => Symbology::Pdf417,
        "qr" => Symbology::Qr,
        "upc-a" => Symbology::UpcA,
        "upc-e" => Symbology::UpcE,
        _ => return None,
    })
}

/// Symbologies Vision serves on this OS, fetched once.
#[cfg(feature = "barcode")]
#[must_use]
pub fn supported_symbologies() -> enumset::EnumSet<crate::Symbology> {
    static SYMBOLOGIES: OnceLock<enumset::EnumSet<crate::Symbology>> = OnceLock::new();
    *SYMBOLOGIES.get_or_init(|| {
        let names: Vec<String> = serde_json::from_str(&ffi::vision_supported_symbologies())
            .expect("the bridge reports a JSON string array");
        names
            .iter()
            .filter_map(|name| symbology_from_name(name))
            .collect()
    })
}

/// Whether Vision serves `requested` exactly.
#[cfg(feature = "barcode")]
pub fn barcodes_offer(requested: enumset::EnumSet<crate::Symbology>) -> Offer {
    let supported = supported_symbologies();
    if supported.is_empty() {
        return Offer::Absent;
    }
    let missing: Vec<&'static str> = (requested - supported).iter().map(name).collect();
    if missing.is_empty() {
        Offer::Serves
    } else {
        Offer::Lacks(format!("symbologies {}", missing.join(", ")))
    }
}

/// Runs barcode detection through Vision on the pass's shared handler.
#[cfg(feature = "barcode")]
pub async fn detect_barcodes(
    pass: &mut Pass<'_>,
    symbologies: enumset::EnumSet<crate::Symbology>,
) -> Result<Vec<crate::Barcode>, VisionError> {
    let handler = pass.prepared::<AppleImage>().await?.handler;
    let names: Vec<&'static str> = symbologies.iter().map(name).collect();
    let json = serde_json::to_string(&names).expect("serializing strings cannot fail");
    let barcodes = ffi_outcome::<WireBarcode>(|callback| {
        ffi::vision_detect_barcodes(handler, &json, callback);
    })
    .await?;
    Ok(barcodes
        .into_iter()
        .map(WireBarcode::into_barcode)
        .collect())
}

/// A barcode as the bridge reports it.
#[cfg(any(feature = "barcode", feature = "document"))]
#[derive(Debug, serde::Deserialize)]
pub struct WireBarcode {
    /// Its canonical symbology name.
    symbology: String,
    /// The decoded payload bytes.
    payload: Vec<u8>,
    /// The four upright corners in reading order, x/y interleaved.
    corners: [f32; 8],
}

#[cfg(any(feature = "barcode", feature = "document"))]
impl WireBarcode {
    pub fn into_barcode(self) -> crate::Barcode {
        crate::Barcode {
            symbology: symbology_from_name(&self.symbology).unwrap_or_else(|| {
                panic!(
                    "the bridge only reports symbologies this vocabulary names; it reported {}",
                    self.symbology
                )
            }),
            payload: crate::Payload {
                bytes: bytes::Bytes::from(self.payload),
            },
            bounds: wire_quad(self.corners),
        }
    }
}

/// A text line as the bridge reports it, shared by the text request and by
/// document paragraphs.
#[cfg(any(feature = "text", feature = "document"))]
#[derive(Debug, serde::Deserialize)]
pub struct WireTextLine {
    /// The recognized text.
    text: String,
    /// Vision's confidence, 0 to 1.
    confidence: f32,
    /// The four upright corners in reading order, x/y interleaved.
    corners: [f32; 8],
    /// The line's words in reading order.
    words: Vec<WireTextWord>,
}

#[cfg(any(feature = "text", feature = "document"))]
impl WireTextLine {
    pub fn into_line(self) -> crate::TextLine {
        crate::TextLine {
            text: self.text,
            confidence: Some(self.confidence),
            bounds: wire_quad(self.corners),
            words: self
                .words
                .into_iter()
                .map(|word| crate::TextWord {
                    text: word.text,
                    confidence: Some(word.confidence),
                    bounds: wire_quad(word.corners),
                })
                .collect(),
        }
    }
}

/// A recognized word as the bridge reports it.
#[cfg(any(feature = "text", feature = "document"))]
#[derive(Debug, serde::Deserialize)]
struct WireTextWord {
    /// The recognized text.
    text: String,
    /// The line candidate's confidence, 0 to 1.
    confidence: f32,
    /// The four upright corners in reading order, x/y interleaved.
    corners: [f32; 8],
}

#[cfg(all(test, feature = "camera", feature = "barcode"))]
mod tests {
    //! The camera `Frame` path end to end: a `CVPixelBuffer` with known QR
    //! pixels is handed to `vision_handler_pixel_buffer` and decodes.

    use std::ptr::NonNull;

    use objc2_core_foundation::{CFDictionary, CFRetained, CFString, CFType};
    use objc2_core_video::{
        CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddress,
        CVPixelBufferGetBytesPerRow, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
        CVPixelBufferUnlockBaseAddress, kCVPixelBufferIOSurfacePropertiesKey,
        kCVPixelFormatType_32BGRA,
    };
    use waterkit_camera::FramePlanes;

    use crate::sys::PixelBuffer;
    use crate::{DetectBarcodes, Image, Orientation, Symbology, Vision, test_support::gpu};

    fn qr() -> ::image::RgbaImage {
        let code = qrcode::QrCode::new(b"captured").expect("content fits a QR code");
        let luma = code
            .render::<::image::Luma<u8>>()
            .min_dimensions(256, 256)
            .build();
        ::image::DynamicImage::ImageLuma8(luma).to_rgba8()
    }

    /// An IOSurface-backed BGRA buffer holding `image`'s pixels, as a
    /// capture delivers.
    fn capture_buffer(image: &::image::RgbaImage) -> CFRetained<CVPixelBuffer> {
        let (width, height) = (image.width() as usize, image.height() as usize);
        // SAFETY: Core Video's immutable attribute key.
        let io_surface = unsafe { kCVPixelBufferIOSurfacePropertiesKey };
        let no_properties = CFDictionary::<CFString, CFType>::empty();
        let attributes =
            CFDictionary::<CFString, CFType>::from_slices(&[io_surface], &[no_properties.as_ref()]);
        let mut buffer: *mut CVPixelBuffer = std::ptr::null_mut();
        // SAFETY: the out pointer is valid for the call; a created buffer
        // carries one reference the `CFRetained` takes over.
        let buffer = unsafe {
            assert_eq!(
                CVPixelBufferCreate(
                    None,
                    width,
                    height,
                    kCVPixelFormatType_32BGRA,
                    Some(attributes.as_opaque()),
                    NonNull::from(&mut buffer),
                ),
                0
            );
            CFRetained::from_raw(NonNull::new(buffer).expect("a created pixel buffer"))
        };

        // SAFETY: the buffer is live; it is locked for the fill and unlocked
        // with the same flags.
        unsafe {
            assert_eq!(
                CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags(0)),
                0
            );
            let base = CVPixelBufferGetBaseAddress(&buffer).cast::<u8>();
            let stride = CVPixelBufferGetBytesPerRow(&buffer);
            for row in 0..height {
                let line = std::slice::from_raw_parts_mut(base.add(row * stride), width * 4);
                for (column, pixel) in line.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    let rgba = image
                        .get_pixel(
                            u32::try_from(column).expect("width fits u32"),
                            u32::try_from(row).expect("height fits u32"),
                        )
                        .0;
                    pixel.copy_from_slice(&[rgba[2], rgba[1], rgba[0], rgba[3]]);
                }
            }
            assert_eq!(
                CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags(0)),
                0
            );
        }
        buffer
    }

    #[test]
    fn a_captured_pixel_buffer_reaches_vision_uncopied() {
        let (device, queue) = gpu();
        let vision = Vision::new(device.clone(), queue);

        // The dummy plane is never read: the frame carries its buffer.
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vision captured-frame test"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let planes = FramePlanes::Rgb(texture.create_view(&wgpu::TextureViewDescriptor::default()));
        // The dummy RGB plane's nominal colour description; the buffer path
        // under test never reads it.
        let color = waterkit_camera::VideoColorInfo {
            matrix: waterkit_camera::MatrixCoefficients::Bt709,
            primaries: waterkit_camera::ColorPrimaries::Bt709,
            transfer: waterkit_camera::TransferFunction::Sdr,
            range: waterkit_camera::ColorRange::Full,
            content_light_level: None,
            dolby_vision: false,
        };
        let mut image = Image::from_planes(&planes, color, Orientation::Up);
        let crate::image::Pixels::Frame { pixel_buffer, .. } = image.pixels_mut() else {
            panic!("a frame image holds frame pixels")
        };
        *pixel_buffer = Some(PixelBuffer(capture_buffer(&qr())));

        let request = DetectBarcodes::new(Symbology::Qr);
        let barcodes = pollster::block_on(vision.perform(&image, &request))
            .expect("the captured buffer decodes");
        assert_eq!(barcodes.len(), 1);
        assert_eq!(barcodes[0].payload().text(), Some("captured"));
    }
}
