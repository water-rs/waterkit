use half::f16;
use image::{ColorType, DynamicImage, GenericImageView};
#[cfg(target_vendor = "apple")]
use std::ffi::c_void;
#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "ios", target_os = "android"))
))]
use std::io::Cursor;
#[cfg(target_vendor = "apple")]
use std::ptr;
#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "ios", target_os = "android"))
))]
use yuv::{YuvBiPlanarImage, YuvConversionMode, YuvRange, YuvStandardMatrix, yuv_nv12_to_rgba};

use crate::CodecError;
#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "ios", target_os = "android"))
))]
use crate::software::av1::Av1Decoder;

#[cfg(target_vendor = "apple")]
#[repr(C)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[cfg(target_vendor = "apple")]
#[repr(C)]
struct CGSize {
    width: f64,
    height: f64,
}

#[cfg(target_vendor = "apple")]
#[repr(C)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[cfg(target_vendor = "apple")]
const K_CG_IMAGE_ALPHA_PREMULTIPLIED_LAST: u32 = 1;
#[cfg(target_vendor = "apple")]
const K_CG_BITMAP_BYTE_ORDER_32_BIG: u32 = 4 << 12;

#[cfg(target_vendor = "apple")]
#[link(name = "CoreFoundation", kind = "framework")]
#[link(name = "CoreGraphics", kind = "framework")]
#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    fn CFDataCreate(allocator: *const c_void, bytes: *const u8, length: isize) -> *const c_void;
    fn CFRelease(cf: *const c_void);

    fn CGImageSourceCreateWithData(data: *const c_void, options: *const c_void) -> *mut c_void;
    fn CGImageSourceCreateImageAtIndex(
        source: *mut c_void,
        index: usize,
        options: *const c_void,
    ) -> *mut c_void;

    fn CGImageGetWidth(image: *const c_void) -> usize;
    fn CGImageGetHeight(image: *const c_void) -> usize;

    fn CGColorSpaceCreateDeviceRGB() -> *const c_void;
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        color_space: *const c_void,
        bitmap_info: u32,
    ) -> *mut c_void;
    fn CGContextDrawImage(context: *mut c_void, rect: CGRect, image: *const c_void);
}

/// Pixel formats currently emitted by `decode_image`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecodedPixelFormat {
    /// 8-bit normalized sRGB RGBA.
    Rgba8UnormSrgb,
    /// 16-bit float RGBA (reserved for HDR decoders).
    Rgba16Float,
}

/// Result of an image decode request.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DecodedImage {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    pixel_format: DecodedPixelFormat,
    hdr: bool,
    wide_gamut: bool,
}

impl DecodedImage {
    /// Create a new `DecodedImage`.
    pub(crate) const fn new(
        pixels: Vec<u8>,
        width: u32,
        height: u32,
        pixel_format: DecodedPixelFormat,
        hdr: bool,
        wide_gamut: bool,
    ) -> Self {
        Self {
            pixels,
            width,
            height,
            pixel_format,
            hdr,
            wide_gamut,
        }
    }

    /// Decoded image pixels in RGBA order.
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Consume this image and return the pixel data.
    #[must_use]
    pub fn into_pixels(self) -> Vec<u8> {
        self.pixels
    }

    /// Decoded image width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Decoded image height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Pixel format describing the pixel data.
    #[must_use]
    pub const fn pixel_format(&self) -> DecodedPixelFormat {
        self.pixel_format
    }

    /// Whether the decoded source is HDR.
    #[must_use]
    pub const fn hdr(&self) -> bool {
        self.hdr
    }

    /// Whether the decoded source uses wide gamut.
    #[must_use]
    pub const fn wide_gamut(&self) -> bool {
        self.wide_gamut
    }
}

/// Decodes image bytes into RGBA pixels.
///
/// # Errors
///
/// Returns [`CodecError::DecodingFailed`] when decoding fails.
pub fn decode_image(data: &[u8]) -> Result<DecodedImage, CodecError> {
    match decode_image_platform(data) {
        Ok(decoded) => Ok(decoded),
        Err(primary_err) => {
            #[cfg(all(
                feature = "software-fallback",
                not(any(target_os = "ios", target_os = "android"))
            ))]
            if is_avif(data) {
                return decode_avif_software(data).map_err(|fallback_err| {
                    CodecError::DecodingFailed(format!(
                        "image decode failed: {primary_err}; AVIF software fallback failed: {fallback_err}"
                    ))
                });
            }
            Err(primary_err)
        }
    }
}

/// Decodes image bytes through the primary image decode path.
///
/// This path intentionally excludes AV1-in-AVIF software fallback logic.
///
/// # Errors
///
/// Returns [`CodecError::DecodingFailed`] when decoding fails.
pub fn decode_image_platform(data: &[u8]) -> Result<DecodedImage, CodecError> {
    #[cfg(target_vendor = "apple")]
    if is_heif_family(data) {
        return decode_heif_apple(data);
    }

    let decoded = image::load_from_memory(data)
        .map_err(|err| CodecError::DecodingFailed(format!("image decode failed: {err}")))?;
    let (width, height) = decoded.dimensions();
    let color = decoded.color();

    if is_high_precision_color(color) {
        // `image` does not reliably expose transfer/gamut metadata for every codec, so keep
        // HDR/wide-gamut flags conservative to avoid false positives.
        let (pixels, has_hdr_headroom) = encode_rgba16f(decoded, color);
        return Ok(DecodedImage::new(
            pixels,
            width,
            height,
            DecodedPixelFormat::Rgba16Float,
            has_hdr_headroom,
            false,
        ));
    }

    Ok(DecodedImage::new(
        decoded.into_rgba8().into_raw(),
        width,
        height,
        DecodedPixelFormat::Rgba8UnormSrgb,
        false,
        false,
    ))
}

#[cfg(target_vendor = "apple")]
fn decode_heif_apple(data: &[u8]) -> Result<DecodedImage, CodecError> {
    let data_len = isize::try_from(data.len())
        .map_err(|_| CodecError::DecodingFailed("HEIF data length is too large".into()))?;
    unsafe {
        let cf_data = CFDataCreate(ptr::null(), data.as_ptr(), data_len);
        if cf_data.is_null() {
            return Err(CodecError::DecodingFailed(
                "CFDataCreate returned null for HEIF source".into(),
            ));
        }

        let source = CGImageSourceCreateWithData(cf_data, ptr::null());
        CFRelease(cf_data);
        if source.is_null() {
            return Err(CodecError::DecodingFailed(
                "CGImageSourceCreateWithData failed for HEIF source".into(),
            ));
        }

        let image = CGImageSourceCreateImageAtIndex(source, 0, ptr::null());
        CFRelease(source as *const c_void);
        if image.is_null() {
            return Err(CodecError::DecodingFailed(
                "CGImageSourceCreateImageAtIndex failed for HEIF source".into(),
            ));
        }

        let width = CGImageGetWidth(image as *const c_void);
        let height = CGImageGetHeight(image as *const c_void);
        if width == 0 || height == 0 {
            CFRelease(image as *const c_void);
            return Err(CodecError::DecodingFailed(
                "Decoded HEIF image has zero width or height".into(),
            ));
        }

        let bytes_per_row = width
            .checked_mul(4)
            .ok_or_else(|| CodecError::DecodingFailed("HEIF bytes_per_row overflow".into()))?;
        let pixel_len = bytes_per_row
            .checked_mul(height)
            .ok_or_else(|| CodecError::DecodingFailed("HEIF pixel buffer overflow".into()))?;
        let mut pixels = vec![0u8; pixel_len];

        let color_space = CGColorSpaceCreateDeviceRGB();
        if color_space.is_null() {
            CFRelease(image as *const c_void);
            return Err(CodecError::DecodingFailed(
                "CGColorSpaceCreateDeviceRGB failed".into(),
            ));
        }

        let bitmap_info = K_CG_IMAGE_ALPHA_PREMULTIPLIED_LAST | K_CG_BITMAP_BYTE_ORDER_32_BIG;
        let context = CGBitmapContextCreate(
            pixels.as_mut_ptr().cast::<c_void>(),
            width,
            height,
            8,
            bytes_per_row,
            color_space,
            bitmap_info,
        );
        CFRelease(color_space);
        if context.is_null() {
            CFRelease(image as *const c_void);
            return Err(CodecError::DecodingFailed(
                "CGBitmapContextCreate failed for HEIF decode".into(),
            ));
        }

        let rect = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: width as f64,
                height: height as f64,
            },
        };
        CGContextDrawImage(context, rect, image as *const c_void);

        CFRelease(context as *const c_void);
        CFRelease(image as *const c_void);

        let width = u32::try_from(width)
            .map_err(|_| CodecError::DecodingFailed("HEIF width does not fit u32".into()))?;
        let height = u32::try_from(height)
            .map_err(|_| CodecError::DecodingFailed("HEIF height does not fit u32".into()))?;

        Ok(DecodedImage::new(
            pixels,
            width,
            height,
            DecodedPixelFormat::Rgba8UnormSrgb,
            false,
            false,
        ))
    }
}

#[cfg(target_vendor = "apple")]
fn is_heif_family(data: &[u8]) -> bool {
    if data.len() < 16 {
        return false;
    }
    let box_size = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
    if box_size < 16 || box_size > data.len() || &data[4..8] != b"ftyp" {
        return false;
    }
    let major = [data[8], data[9], data[10], data[11]];
    if is_heif_brand(&major) {
        return true;
    }
    let compat = &data[16..box_size];
    if compat.len() % 4 != 0 {
        return false;
    }
    compat
        .chunks_exact(4)
        .any(|brand| is_heif_brand(&[brand[0], brand[1], brand[2], brand[3]]))
}

#[cfg(target_vendor = "apple")]
const fn is_heif_brand(brand: &[u8; 4]) -> bool {
    matches!(
        brand,
        b"mif1" | b"msf1" | b"heif" | b"heic" | b"heix" | b"hevc" | b"hevx"
    )
}

#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "ios", target_os = "android"))
))]
fn is_avif(data: &[u8]) -> bool {
    matches!(image::guess_format(data), Ok(image::ImageFormat::Avif))
}

#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "ios", target_os = "android"))
))]
fn decode_avif_software(data: &[u8]) -> Result<DecodedImage, CodecError> {
    let mut cursor = Cursor::new(data);
    let avif = avif_parse::AvifData::from_reader(&mut cursor)
        .map_err(|err| CodecError::DecodingFailed(format!("AVIF parse failed: {err}")))?;
    let metadata = avif.primary_item_metadata().map_err(|err| {
        CodecError::DecodingFailed(format!("AV1 sequence metadata parse failed: {err}"))
    })?;
    if !(8..=16).contains(&metadata.bit_depth) {
        return Err(CodecError::Unsupported(format!(
            "AVIF software decode supports 8-bit to 16-bit AV1 payloads, got {}-bit",
            metadata.bit_depth
        )));
    }

    let mut primary_decoder = Av1Decoder::new()?;
    let primary_frame = primary_decoder
        .decode(&avif.primary_item)?
        .into_iter()
        .next()
        .ok_or_else(|| CodecError::DecodingFailed("AVIF primary item produced no frame".into()))?;

    let width = primary_frame.width;
    let height = primary_frame.height;
    let uv_width = (width as usize).div_ceil(2);
    let uv_height = (height as usize).div_ceil(2);
    let y_size = (width as usize) * (height as usize);
    let uv_size = uv_width * uv_height * 2;
    if primary_frame.data.len() != y_size + uv_size {
        return Err(CodecError::DecodingFailed(format!(
            "AV1 primary frame NV12 size mismatch: got {}, expected {}",
            primary_frame.data.len(),
            y_size + uv_size
        )));
    }

    let mut rgba = vec![0u8; y_size * 4];
    let bi_planar = YuvBiPlanarImage {
        y_plane: &primary_frame.data[..y_size],
        y_stride: width,
        uv_plane: &primary_frame.data[y_size..],
        uv_stride: (uv_width * 2) as u32,
        width,
        height,
    };
    yuv_nv12_to_rgba(
        &bi_planar,
        &mut rgba,
        width * 4,
        YuvRange::Full,
        YuvStandardMatrix::Bt709,
        YuvConversionMode::Balanced,
    )
    .map_err(|err| CodecError::DecodingFailed(format!("NV12 to RGBA conversion failed: {err}")))?;

    if let Some(alpha_item) = avif.alpha_item.as_deref() {
        let mut alpha_decoder = Av1Decoder::new()?;
        let alpha_frame = alpha_decoder
            .decode(alpha_item)?
            .into_iter()
            .next()
            .ok_or_else(|| {
                CodecError::DecodingFailed("AVIF alpha item produced no frame".into())
            })?;
        if alpha_frame.width != width || alpha_frame.height != height {
            return Err(CodecError::DecodingFailed(format!(
                "AVIF alpha frame dimensions mismatch: alpha={}x{}, primary={}x{}",
                alpha_frame.width, alpha_frame.height, width, height
            )));
        }
        if alpha_frame.data.len() < y_size {
            return Err(CodecError::DecodingFailed(format!(
                "AVIF alpha frame Y plane too small: got {}, need at least {}",
                alpha_frame.data.len(),
                y_size
            )));
        }
        for (idx, alpha) in alpha_frame.data[..y_size].iter().enumerate() {
            rgba[idx * 4 + 3] = *alpha;
        }
    }

    if metadata.bit_depth > 8 {
        let rgba16f = encode_rgba8_to_hdr_rgba16f(&rgba, metadata.bit_depth);
        return Ok(DecodedImage::new(
            rgba16f,
            width,
            height,
            DecodedPixelFormat::Rgba16Float,
            true,
            false,
        ));
    }

    Ok(DecodedImage::new(
        rgba,
        width,
        height,
        DecodedPixelFormat::Rgba8UnormSrgb,
        false,
        false,
    ))
}

#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "ios", target_os = "android"))
))]
fn encode_rgba8_to_hdr_rgba16f(rgba8: &[u8], bit_depth: u8) -> Vec<u8> {
    let max_code = ((1u32 << u32::from(bit_depth)) - 1) as f32;
    let headroom_scale = max_code / 255.0;
    let mut out = Vec::with_capacity(rgba8.len() * core::mem::size_of::<u16>());
    for px in rgba8.chunks_exact(4) {
        let r = (f32::from(px[0]) / 255.0) * headroom_scale;
        let g = (f32::from(px[1]) / 255.0) * headroom_scale;
        let b = (f32::from(px[2]) / 255.0) * headroom_scale;
        let a = f32::from(px[3]) / 255.0;
        out.extend_from_slice(&f16::from_f32(r).to_le_bytes());
        out.extend_from_slice(&f16::from_f32(g).to_le_bytes());
        out.extend_from_slice(&f16::from_f32(b).to_le_bytes());
        out.extend_from_slice(&f16::from_f32(a).to_le_bytes());
    }
    out
}

const fn is_high_precision_color(color: ColorType) -> bool {
    matches!(color, ColorType::Rgb32F | ColorType::Rgba32F)
}

fn encode_rgba16f(image: DynamicImage, color: ColorType) -> (Vec<u8>, bool) {
    let mut output = Vec::new();
    let mut has_hdr_headroom = false;
    match color {
        ColorType::Rgb32F | ColorType::Rgba32F => {
            let rgba = image.into_rgba32f().into_raw();
            output.reserve(rgba.len() * core::mem::size_of::<u16>());
            for channel in rgba {
                has_hdr_headroom |= channel.is_finite() && channel > 1.0;
                output.extend_from_slice(&f16::from_f32(channel).to_le_bytes());
            }
        }
        _ => unreachable!("encode_rgba16f only supports 32-bit float inputs"),
    }
    (output, has_hdr_headroom)
}
