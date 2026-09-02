use half::f16;
use image::{ColorType, DynamicImage, GenericImageView};
#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "android", target_arch = "wasm32")),
    any(test, not(target_vendor = "apple"))
))]
use moxcms::{
    CicpColorPrimaries, CicpProfile, ColorProfile, Layout, MatrixCoefficients as CicpMatrix,
    TransferCharacteristics, TransformOptions,
};
#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "android", target_arch = "wasm32")),
    any(test, not(target_vendor = "apple"))
))]
use std::io::Cursor;
#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "android", target_arch = "wasm32")),
    any(test, not(target_vendor = "apple"))
))]
use yuv::{
    YuvBiPlanarImage, YuvConversionMode, YuvRange, YuvStandardMatrix, p010_to_rgba10,
    yuv_nv12_to_rgba,
};

use crate::CodecError;
#[cfg(target_vendor = "apple")]
use crate::image_apple;
#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "android", target_arch = "wasm32")),
    any(test, not(target_vendor = "apple"))
))]
use crate::software::av1::{Av1Decoder, CpuFrame};
#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "android", target_arch = "wasm32")),
    any(test, not(target_vendor = "apple"))
))]
use crate::{DecodedPixelLayout, SDR_REFERENCE_WHITE_NITS};

/// Pixel formats currently emitted by `decode_image`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecodedPixelFormat {
    /// 8-bit normalized sRGB RGBA.
    Rgba8UnormSrgb,
    /// 16-bit floating-point linear RGBA.
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
    not(any(target_os = "android", target_arch = "wasm32")),
    any(test, not(target_vendor = "apple"))
))]
pub fn decode_image(data: &[u8]) -> Result<DecodedImage, CodecError> {
    if is_avif(data) {
        #[cfg(target_vendor = "apple")]
        return decode_image_platform(data);
        #[cfg(not(target_vendor = "apple"))]
        return decode_avif_software(data);
    }
    decode_image_platform(data)
}

/// Decodes image bytes into RGBA pixels.
///
/// # Errors
///
/// Returns [`CodecError::DecodingFailed`] when decoding fails.
#[cfg(not(all(
    feature = "software-fallback",
    not(any(target_os = "android", target_arch = "wasm32")),
    any(test, not(target_vendor = "apple"))
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
    not(any(target_os = "android", target_arch = "wasm32")),
    any(test, not(target_vendor = "apple"))
))]
fn is_avif(data: &[u8]) -> bool {
    matches!(image::guess_format(data), Ok(image::ImageFormat::Avif))
}

#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "android", target_arch = "wasm32")),
    any(test, not(target_vendor = "apple"))
))]
fn decode_avif_software(data: &[u8]) -> Result<DecodedImage, CodecError> {
    let mut cursor = Cursor::new(data);
    let avif = avif_parse::AvifData::from_reader(&mut cursor)
        .map_err(|err| CodecError::DecodingFailed(format!("AVIF parse failed: {err}")))?;
    let metadata = avif.primary_item_metadata().map_err(|err| {
        CodecError::DecodingFailed(format!("AV1 sequence metadata parse failed: {err}"))
    })?;
    if !matches!(metadata.bit_depth, 8 | 10) {
        return Err(CodecError::Unsupported(format!(
            "AVIF software decode supports 8-bit and 10-bit AV1 payloads, got {}-bit",
            metadata.bit_depth
        )));
    }

    let primary_frame = decode_av1_item(&avif.primary_item, "primary")?;
    let expected_layout = if metadata.bit_depth == 8 {
        DecodedPixelLayout::Nv12
    } else {
        DecodedPixelLayout::P010
    };
    if primary_frame.layout != expected_layout {
        return Err(CodecError::DecodingFailed(format!(
            "AVIF metadata declares {}-bit samples but rav1d returned {:?}",
            metadata.bit_depth, primary_frame.layout
        )));
    }
    let width = primary_frame.width;
    let height = primary_frame.height;
    let transfer = normalized_transfer(primary_frame.color.transfer)?;
    let hdr = is_hdr_transfer(transfer);
    let wide_gamut =
        normalized_primaries(primary_frame.color.primaries)? != CicpColorPrimaries::Bt709;
    let mut linear_rgba = decode_avif_linear_rgba(&primary_frame)?;

    if let Some(alpha_item) = avif.alpha_item.as_deref() {
        apply_avif_alpha(&mut linear_rgba, alpha_item, width, height)?;
    }

    if hdr {
        let rgba16f = encode_linear_rgba16f(&linear_rgba);
        return Ok(DecodedImage::new(
            rgba16f,
            width,
            height,
            DecodedPixelFormat::Rgba16Float,
            true,
            wide_gamut,
        ));
    }

    Ok(DecodedImage::new(
        encode_linear_srgb_rgba8(&linear_rgba),
        width,
        height,
        DecodedPixelFormat::Rgba8UnormSrgb,
        false,
        wide_gamut,
    ))
}

#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "android", target_arch = "wasm32")),
    any(test, not(target_vendor = "apple"))
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
    not(any(target_os = "android", target_arch = "wasm32")),
    any(test, not(target_vendor = "apple"))
))]
fn apply_avif_alpha(
    rgba: &mut [f32],
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
    let sample_bytes = match alpha_frame.layout {
        DecodedPixelLayout::Nv12 => 1,
        DecodedPixelLayout::P010 => 2,
    };
    let y_len = pixel_count
        .checked_mul(sample_bytes)
        .ok_or_else(|| CodecError::DecodingFailed("AVIF alpha plane length overflow".into()))?;
    if alpha_frame.data.len() < y_len {
        return Err(CodecError::DecodingFailed(format!(
            "AVIF alpha frame Y plane too small: got {}, need at least {y_len}",
            alpha_frame.data.len()
        )));
    }
    for (index, pixel) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        pixel[3] = match alpha_frame.layout {
            DecodedPixelLayout::Nv12 => f32::from(alpha_frame.data[index]) / 255.0,
            DecodedPixelLayout::P010 => {
                let offset = index * 2;
                f32::from(
                    u16::from_le_bytes([alpha_frame.data[offset], alpha_frame.data[offset + 1]])
                        >> 6,
                ) / 1023.0
            }
        };
    }
    Ok(())
}

#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "android", target_arch = "wasm32")),
    any(test, not(target_vendor = "apple"))
))]
fn decode_avif_linear_rgba(frame: &CpuFrame) -> Result<Vec<f32>, CodecError> {
    let width = frame.width;
    let height = frame.height;
    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| CodecError::DecodingFailed("AVIF dimensions overflow usize".into()))?;
    let encoded_rgba = decode_avif_yuv(frame, pixel_count)?;
    let transfer = normalized_transfer(frame.color.transfer)?;
    let source = ColorProfile::new_from_cicp(CicpProfile {
        color_primaries: normalized_primaries(frame.color.primaries)?,
        transfer_characteristics: transfer,
        matrix_coefficients: CicpMatrix::Identity,
        full_range: true,
    });
    let destination = ColorProfile::new_from_cicp(CicpProfile {
        color_primaries: CicpColorPrimaries::Bt709,
        transfer_characteristics: TransferCharacteristics::Linear,
        matrix_coefficients: CicpMatrix::Identity,
        full_range: true,
    });
    let transform = source
        .create_transform_f32(
            Layout::Rgba,
            &destination,
            Layout::Rgba,
            TransformOptions {
                allow_extended_range_rgb_xyz: true,
                ..TransformOptions::default()
            },
        )
        .map_err(|err| CodecError::DecodingFailed(format!("AVIF CICP transform failed: {err}")))?;
    let mut linear = vec![0.0; encoded_rgba.len()];
    transform
        .transform(&encoded_rgba, &mut linear)
        .map_err(|err| CodecError::DecodingFailed(format!("AVIF color transform failed: {err}")))?;

    for pixel in linear.as_chunks_mut::<4>().0 {
        let absolute_scale = match transfer {
            TransferCharacteristics::Smpte2084 => 10_000.0 / SDR_REFERENCE_WHITE_NITS,
            TransferCharacteristics::Hlg => {
                let luminance =
                    0.0722_f32.mul_add(pixel[2], 0.7152_f32.mul_add(pixel[1], 0.2126 * pixel[0]));
                luminance.max(1e-6).powf(0.2) * (1_000.0 / SDR_REFERENCE_WHITE_NITS)
            }
            _ => 1.0,
        };
        pixel[0] *= absolute_scale;
        pixel[1] *= absolute_scale;
        pixel[2] *= absolute_scale;
    }
    Ok(linear)
}

#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "android", target_arch = "wasm32")),
    any(test, not(target_vendor = "apple"))
))]
fn decode_avif_yuv(frame: &CpuFrame, pixel_count: usize) -> Result<Vec<f32>, CodecError> {
    let expected_len = frame.layout.packed_len(frame.width, frame.height);
    if frame.data.len() != expected_len {
        return Err(CodecError::DecodingFailed(format!(
            "AV1 primary frame {:?} size mismatch: got {}, expected {expected_len}",
            frame.layout,
            frame.data.len()
        )));
    }
    let range = if frame.color.full_range {
        YuvRange::Full
    } else {
        YuvRange::Limited
    };
    let matrix = match normalized_matrix(frame.color.matrix)? {
        CicpMatrix::Bt709 => YuvStandardMatrix::Bt709,
        CicpMatrix::Bt470Bg | CicpMatrix::Smpte170m => YuvStandardMatrix::Bt601,
        CicpMatrix::Bt2020Ncl => YuvStandardMatrix::Bt2020,
        matrix => {
            return Err(CodecError::Unsupported(format!(
                "AVIF software decode does not support CICP matrix {matrix:?}"
            )));
        }
    };
    let y_samples = pixel_count;
    match frame.layout {
        DecodedPixelLayout::Nv12 => {
            let y_size = y_samples;
            let mut rgba = vec![0_u8; pixel_count * 4];
            let image = YuvBiPlanarImage {
                y_plane: &frame.data[..y_size],
                y_stride: frame.width,
                uv_plane: &frame.data[y_size..],
                uv_stride: frame.width.div_ceil(2) * 2,
                width: frame.width,
                height: frame.height,
            };
            yuv_nv12_to_rgba(
                &image,
                &mut rgba,
                frame.width * 4,
                range,
                matrix,
                YuvConversionMode::Balanced,
            )
            .map_err(|err| {
                CodecError::DecodingFailed(format!("NV12 to RGBA conversion failed: {err}"))
            })?;
            Ok(rgba
                .into_iter()
                .map(|value| f32::from(value) / 255.0)
                .collect())
        }
        DecodedPixelLayout::P010 => {
            let samples: Vec<u16> = frame
                .data
                .as_chunks::<2>()
                .0
                .iter()
                .map(|bytes| u16::from_le_bytes(*bytes))
                .collect();
            let mut rgba = vec![0_u16; pixel_count * 4];
            let image = YuvBiPlanarImage {
                y_plane: &samples[..y_samples],
                y_stride: frame.width,
                uv_plane: &samples[y_samples..],
                uv_stride: frame.width.div_ceil(2) * 2,
                width: frame.width,
                height: frame.height,
            };
            p010_to_rgba10(&image, &mut rgba, frame.width * 4, range, matrix).map_err(|err| {
                CodecError::DecodingFailed(format!("P010 to RGBA10 conversion failed: {err}"))
            })?;
            Ok(rgba
                .into_iter()
                .map(|value| f32::from(value) / 1023.0)
                .collect())
        }
    }
}

#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "android", target_arch = "wasm32")),
    any(test, not(target_vendor = "apple"))
))]
fn normalized_primaries(value: u8) -> Result<CicpColorPrimaries, CodecError> {
    let primaries = CicpColorPrimaries::try_from(value).map_err(|err| {
        CodecError::DecodingFailed(format!("invalid AVIF color primaries: {err}"))
    })?;
    let primaries = match primaries {
        CicpColorPrimaries::Unspecified => CicpColorPrimaries::Bt709,
        value => value,
    };
    match primaries {
        CicpColorPrimaries::Bt709
        | CicpColorPrimaries::Bt470Bg
        | CicpColorPrimaries::Bt601
        | CicpColorPrimaries::Bt2020
        | CicpColorPrimaries::Smpte432 => Ok(primaries),
        value => Err(CodecError::Unsupported(format!(
            "AVIF software decode does not support CICP color primaries {value:?}"
        ))),
    }
}

#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "android", target_arch = "wasm32")),
    any(test, not(target_vendor = "apple"))
))]
fn normalized_transfer(value: u8) -> Result<TransferCharacteristics, CodecError> {
    let transfer = TransferCharacteristics::try_from(value).map_err(|err| {
        CodecError::DecodingFailed(format!("invalid AVIF transfer function: {err}"))
    })?;
    let transfer = match transfer {
        TransferCharacteristics::Unspecified => TransferCharacteristics::Srgb,
        value => value,
    };
    match transfer {
        TransferCharacteristics::Bt709
        | TransferCharacteristics::Bt601
        | TransferCharacteristics::Linear
        | TransferCharacteristics::Srgb
        | TransferCharacteristics::Bt202010bit
        | TransferCharacteristics::Bt202012bit
        | TransferCharacteristics::Smpte2084
        | TransferCharacteristics::Hlg => Ok(transfer),
        value => Err(CodecError::Unsupported(format!(
            "AVIF software decode does not support CICP transfer function {value:?}"
        ))),
    }
}

#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "android", target_arch = "wasm32")),
    any(test, not(target_vendor = "apple"))
))]
const fn is_hdr_transfer(transfer: TransferCharacteristics) -> bool {
    matches!(
        transfer,
        TransferCharacteristics::Smpte2084 | TransferCharacteristics::Hlg
    )
}

#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "android", target_arch = "wasm32")),
    any(test, not(target_vendor = "apple"))
))]
fn normalized_matrix(value: u8) -> Result<CicpMatrix, CodecError> {
    let matrix = CicpMatrix::try_from(value)
        .map_err(|err| CodecError::DecodingFailed(format!("invalid AVIF matrix: {err}")))?;
    Ok(match matrix {
        CicpMatrix::Unspecified => CicpMatrix::Bt709,
        value => value,
    })
}

#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "android", target_arch = "wasm32")),
    any(test, not(target_vendor = "apple"))
))]
fn encode_linear_rgba16f(rgba: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len() * core::mem::size_of::<u16>());
    for channel in rgba {
        out.extend_from_slice(&f16::from_f32(*channel).to_le_bytes());
    }
    out
}

#[cfg(all(
    feature = "software-fallback",
    not(any(target_os = "android", target_arch = "wasm32")),
    any(test, not(target_vendor = "apple"))
))]
fn encode_linear_srgb_rgba8(rgba: &[f32]) -> Vec<u8> {
    let encode_unorm8 = |value: f32| {
        let rounded = (value.clamp(0.0, 1.0) * 255.0).round();
        // SAFETY: clamping and scaling constrain the finite result to the inclusive u8 range.
        unsafe { rounded.to_int_unchecked::<u8>() }
    };
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|pixel| {
            let encode = |value: f32| {
                let encoded = if value <= 0.003_130_8 {
                    value * 12.92
                } else {
                    1.055_f32.mul_add(value.powf(1.0 / 2.4), -0.055)
                };
                encode_unorm8(encoded)
            };
            [
                encode(pixel[0]),
                encode(pixel[1]),
                encode(pixel[2]),
                encode_unorm8(pixel[3]),
            ]
        })
        .collect()
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

#[cfg(all(
    test,
    feature = "software-fallback",
    not(any(target_os = "android", target_arch = "wasm32")),
    any(test, not(target_vendor = "apple"))
))]
mod tests {
    use image::{ExtendedColorType, ImageEncoder, codecs::avif::AvifEncoder};
    use moxcms::TransferCharacteristics;

    use super::{
        DecodedPixelFormat, decode_avif_software, encode_linear_srgb_rgba8, is_hdr_transfer,
    };

    #[test]
    fn software_avif_decoder_runs_end_to_end() {
        let pixels = [
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let mut avif = Vec::new();
        AvifEncoder::new_with_speed_quality(&mut avif, 10, 100)
            .with_num_threads(Some(1))
            .write_image(&pixels, 2, 2, ExtendedColorType::Rgba8)
            .expect("test AVIF encoding must succeed");

        let decoded = decode_avif_software(&avif).expect("software AVIF decoding must succeed");
        assert_eq!(decoded.width(), 2);
        assert_eq!(decoded.height(), 2);
        assert_eq!(decoded.pixel_format(), DecodedPixelFormat::Rgba8UnormSrgb);
        assert!(!decoded.hdr());
        assert_eq!(decoded.pixels().len(), pixels.len());
    }

    #[test]
    fn hdr_classification_uses_transfer_function_not_sample_precision() {
        assert!(!is_hdr_transfer(TransferCharacteristics::Srgb));
        assert!(!is_hdr_transfer(TransferCharacteristics::Bt202010bit));
        assert!(is_hdr_transfer(TransferCharacteristics::Smpte2084));
        assert!(is_hdr_transfer(TransferCharacteristics::Hlg));
    }

    #[test]
    fn linear_sdr_is_encoded_as_srgb() {
        let encoded = encode_linear_srgb_rgba8(&[0.5, 0.5, 0.5, 1.0]);
        assert_eq!(encoded, [188, 188, 188, 255]);
    }
}
