use half::f16;
use image::{ColorType, DynamicImage, GenericImageView};
#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
))]
use std::io::Cursor;
#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
))]
use yuv::{YuvBiPlanarImage, YuvConversionMode, YuvRange, YuvStandardMatrix, yuv_nv12_to_rgba};

use crate::CodecError;
#[cfg(target_vendor = "apple")]
use crate::image_apple;
#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
))]
use crate::software::av1::{Av1Decoder, CpuFrame};

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
#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
))]
pub fn decode_image(data: &[u8]) -> Result<DecodedImage, CodecError> {
    match decode_image_platform(data) {
        Ok(decoded) => Ok(decoded),
        Err(primary_err) => {
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

/// Decodes image bytes into RGBA pixels.
///
/// # Errors
///
/// Returns [`CodecError::DecodingFailed`] when decoding fails.
#[cfg(not(all(
    feature = "software-fallback",
    not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
)))]
pub fn decode_image(data: &[u8]) -> Result<DecodedImage, CodecError> {
    decode_image_platform(data)
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
    if is_avif_family(data) {
        return decode_avif_apple(data);
    }

    #[cfg(target_vendor = "apple")]
    if is_heif_family(data) {
        return decode_isobmff_apple(data);
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
fn decode_avif_apple(data: &[u8]) -> Result<DecodedImage, CodecError> {
    if !image_apple::is_av1_hardware_decode_supported() {
        return Err(CodecError::Unsupported(
            "AV1 hardware decode is unavailable on this Apple device".into(),
        ));
    }
    decode_isobmff_apple(data)
}

#[cfg(target_vendor = "apple")]
fn decode_isobmff_apple(data: &[u8]) -> Result<DecodedImage, CodecError> {
    let decoded = image_apple::decode_isobmff_image(data)?;
    let pixel_format = match decoded.pixel_format {
        image_apple::AppleDecodedPixelFormat::Rgba8UnormSrgb => DecodedPixelFormat::Rgba8UnormSrgb,
        image_apple::AppleDecodedPixelFormat::Rgba16Float => DecodedPixelFormat::Rgba16Float,
    };
    Ok(DecodedImage::new(
        decoded.pixels,
        decoded.width,
        decoded.height,
        pixel_format,
        decoded.hdr,
        false,
    ))
}

#[cfg(target_vendor = "apple")]
fn is_heif_family(data: &[u8]) -> bool {
    let Some((major, compat)) = parse_isobmff_ftyp(data) else {
        return false;
    };

    if is_avif_brand(major) {
        return false;
    }

    if compat
        .as_chunks::<4>()
        .0
        .iter()
        .any(|brand| is_avif_brand(*brand))
    {
        return false;
    }
    if is_heif_brand(major) {
        return true;
    }
    compat
        .as_chunks::<4>()
        .0
        .iter()
        .any(|brand| is_heif_brand(*brand))
}

#[cfg(target_vendor = "apple")]
fn is_avif_family(data: &[u8]) -> bool {
    let Some((major, compat)) = parse_isobmff_ftyp(data) else {
        return false;
    };
    if is_avif_brand(major) {
        return true;
    }
    compat
        .as_chunks::<4>()
        .0
        .iter()
        .any(|brand| is_avif_brand(*brand))
}

#[cfg(target_vendor = "apple")]
fn parse_isobmff_ftyp(data: &[u8]) -> Option<([u8; 4], &[u8])> {
    if data.len() < 16 {
        return None;
    }
    let box_size =
        usize::try_from(u32::from_be_bytes([data[0], data[1], data[2], data[3]])).ok()?;
    if box_size < 16 || box_size > data.len() || &data[4..8] != b"ftyp" {
        return None;
    }
    let compat = &data[16..box_size];
    if !compat.len().is_multiple_of(4) {
        return None;
    }
    Some(([data[8], data[9], data[10], data[11]], compat))
}

#[cfg(target_vendor = "apple")]
fn is_heif_brand(brand: [u8; 4]) -> bool {
    brand == *b"mif1"
        || brand == *b"msf1"
        || brand == *b"heif"
        || brand == *b"heic"
        || brand == *b"heix"
        || brand == *b"hevc"
        || brand == *b"hevx"
}

#[cfg(target_vendor = "apple")]
fn is_avif_brand(brand: [u8; 4]) -> bool {
    brand == *b"avif" || brand == *b"avis"
}

#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
))]
fn is_avif(data: &[u8]) -> bool {
    matches!(image::guess_format(data), Ok(image::ImageFormat::Avif))
}

#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
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

    let primary_frame = decode_av1_item(&avif.primary_item, "primary")?;

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
    let uv_stride = u32::try_from(
        uv_width
            .checked_mul(2)
            .ok_or_else(|| CodecError::DecodingFailed("AVIF UV stride overflow".to_string()))?,
    )
    .map_err(|_| CodecError::DecodingFailed("AVIF UV stride exceeds u32 range".to_string()))?;
    let bi_planar = YuvBiPlanarImage {
        y_plane: &primary_frame.data[..y_size],
        y_stride: width,
        uv_plane: &primary_frame.data[y_size..],
        uv_stride,
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
        apply_avif_alpha(&mut rgba, alpha_item, width, height)?;
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
    not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
))]
fn decode_av1_item(data: &[u8], item_name: &str) -> Result<CpuFrame, CodecError> {
    let mut decoder = Av1Decoder::new()?;
    decoder
        .decode(crate::DecodePacket::new(data, std::time::Duration::ZERO))?
        .into_iter()
        .next()
        .ok_or_else(|| {
            CodecError::DecodingFailed(format!("AVIF {item_name} item produced no frame"))
        })
}

#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
))]
fn apply_avif_alpha(
    rgba: &mut [u8],
    alpha_item: &[u8],
    width: u32,
    height: u32,
) -> Result<(), CodecError> {
    let alpha_frame = decode_av1_item(alpha_item, "alpha")?;
    if alpha_frame.width != width || alpha_frame.height != height {
        return Err(CodecError::DecodingFailed(format!(
            "AVIF alpha frame dimensions mismatch: alpha={}x{}, primary={width}x{height}",
            alpha_frame.width, alpha_frame.height
        )));
    }
    let pixel_count = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or_else(|| CodecError::DecodingFailed("AVIF dimensions overflow usize".into()))?;
    if alpha_frame.data.len() < pixel_count {
        return Err(CodecError::DecodingFailed(format!(
            "AVIF alpha frame Y plane too small: got {}, need at least {pixel_count}",
            alpha_frame.data.len()
        )));
    }
    for (pixel, alpha) in rgba
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(&alpha_frame.data[..pixel_count])
    {
        pixel[3] = *alpha;
    }
    Ok(())
}

#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
))]
fn encode_rgba8_to_hdr_rgba16f(rgba8: &[u8], bit_depth: u8) -> Vec<u8> {
    let max_code = 2f32.powi(i32::from(bit_depth)) - 1.0;
    let headroom_scale = max_code / 255.0;
    let mut out = Vec::with_capacity(rgba8.len() * core::mem::size_of::<u16>());
    for px in rgba8.as_chunks::<4>().0 {
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
