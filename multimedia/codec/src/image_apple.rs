//! Apple still-image decode backed by `ImageIO`, `CoreImage` and
//! `VideoToolbox`.
//!
//! `ImageIO` proves the bytes a decodable image, `CoreImage` renders the
//! `CIImage` into a linear extended-sRGB `RGBA` half-float bitmap, and the
//! buffer is reported as HDR `Rgba16Float` when any channel clears the SDR
//! headroom threshold or tonemapped to `Rgba8UnormSrgb` otherwise.
//! `kCMVideoCodecType_AV1` hardware decode support is reported by
//! `VideoToolbox` on iOS 17 / macOS 14 and later.

use core::ptr::NonNull;

use half::f16;
use objc2::available;
use objc2::runtime::AnyObject;
use objc2_core_foundation::{CFData, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGColorSpace, kCGColorSpaceExtendedLinearSRGB};
use objc2_core_image::{
    CIContext, CIContextOption, CIImage, kCIContextOutputColorSpace, kCIContextWorkingColorSpace,
    kCIFormatRGBAh,
};
use objc2_core_media::kCMVideoCodecType_AV1;
use objc2_foundation::{NSData, NSDictionary};
use objc2_image_io::CGImageSource;
use objc2_video_toolbox::VTIsHardwareDecodeSupported;

use crate::CodecError;

/// A decoded pixel channel above this linear value counts as HDR headroom.
const HDR_HEADROOM_THRESHOLD: f32 = 1.001;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppleDecodedPixelFormat {
    Rgba8UnormSrgb,
    Rgba16Float,
}

#[derive(Debug)]
pub struct AppleDecodedImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub pixel_format: AppleDecodedPixelFormat,
    pub hdr: bool,
}

/// The single error every early-out in `decode_isobmff_image` reports; the
/// steps below distinguish nothing the caller could act on.
fn invalid() -> CodecError {
    CodecError::DecodingFailed("ISOBMFF decode returned an invalid image".into())
}

/// Whether `VideoToolbox` reports hardware AV1 decode on this system.
pub fn is_av1_hardware_decode_supported() -> bool {
    // AV1 is a valid `VTIsHardwareDecodeSupported` codec type only on iOS 17 /
    // macOS 14 / tvOS 17 / visionOS 1.0 and later; earlier systems would only
    // report support for older codecs. `..` keeps any other Apple platform
    // true, mirroring the `#available`'s `*` arm.
    if available!(ios = 17.0, macos = 14.0, tvos = 17.0, visionos = 1.0, ..) {
        // SAFETY: stateless framework query guarded by the runtime
        // availability check above.
        unsafe { VTIsHardwareDecodeSupported(kCMVideoCodecType_AV1) }
    } else {
        false
    }
}

/// Integral extent dimension → `u32`, or `None` when it is not within
/// `0 < dimension <= u32::MAX`. `!(dimension > 0)` also rejects NaN.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is integral and guarded to 0 < d <= u32::MAX above"
)]
fn checked_dimension(dimension: f64) -> Option<u32> {
    if dimension.is_nan() || dimension <= 0.0 || dimension > f64::from(u32::MAX) {
        return None;
    }
    Some(dimension as u32)
}

/// Decodes an ISOBMFF-family still image (HEIC, AVIF, …) into `RGBA` pixels.
///
/// # Errors
/// Returns `CodecError::DecodingFailed` when the bytes cannot be decoded.
pub fn decode_isobmff_image(data: &[u8]) -> Result<AppleDecodedImage, CodecError> {
    if data.is_empty() {
        return Err(invalid());
    }

    // One data object feeds both `ImageIO` and `CoreImage`; `NSData`/`CFData`
    // are toll-free bridged so the same buffer serves both APIs.
    let ns_data = NSData::with_bytes(data);
    // SAFETY: `CFData` is `NSData`'s toll-free bridged CoreFoundation twin;
    // an immutable `NSData` reads as an immutable `CFData`.
    let cf_data = unsafe { NonNull::from(&*ns_data).cast::<CFData>().as_ref() };

    // SAFETY: `cf_data` is a live `CFData`; `None` requests the default
    // options.
    let source = unsafe { CGImageSource::with_data(cf_data, None) }.ok_or_else(invalid)?;
    // SAFETY: `source` is a live `CGImageSource` created above.
    let _probe = unsafe { source.image_at_index(0, None) }.ok_or_else(invalid)?;

    // SAFETY: `ns_data` is a live `NSData` created above.
    let ci_image = unsafe { CIImage::imageWithData(&ns_data) }.ok_or_else(invalid)?;

    // SAFETY: read-only accessor on a live `CIImage`.
    let extent = unsafe { ci_image.extent() };
    // `CGRectIntegral`: the smallest integer-coordinate rectangle containing
    // `extent`. A `CIImage` made from data has a finite extent; the guard in
    // `checked_dimension` rejects a NaN or oversized dimension.
    let width = (extent.origin.x + extent.size.width).ceil() - extent.origin.x.floor();
    let height = (extent.origin.y + extent.size.height).ceil() - extent.origin.y.floor();
    let Some(width) = checked_dimension(width) else {
        return Err(invalid());
    };
    let Some(height) = checked_dimension(height) else {
        return Err(invalid());
    };

    let Some(bytes_per_row) = u64::from(width)
        .checked_mul(8)
        .and_then(|row| isize::try_from(row).ok())
    else {
        return Err(invalid());
    };
    let Some(byte_count) = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(8))
        .and_then(|count| usize::try_from(count).ok())
    else {
        return Err(invalid());
    };

    // SAFETY: `kCGColorSpaceExtendedLinearSRGB` is a constant string exported
    // by CoreGraphics.
    let color_space = unsafe { CGColorSpace::with_name(Some(kCGColorSpaceExtendedLinearSRGB)) }
        .ok_or_else(invalid)?;

    // SAFETY: the option keys are constant strings exported by CoreImage.
    let options = unsafe {
        NSDictionary::<CIContextOption, AnyObject>::from_slices(
            &[kCIContextWorkingColorSpace, kCIContextOutputColorSpace],
            &[color_space.as_ref(), color_space.as_ref()],
        )
    };
    // SAFETY: `options` maps `CIContextOption` keys to `CGColorSpace` values,
    // exactly the types `contextWithOptions:` documents.
    let context = unsafe { CIContext::contextWithOptions(Some(&options)) };

    let mut rgba16f = vec![0u8; byte_count];
    // SAFETY: `rgba16f` is writable for `byte_count` bytes and `bytes_per_row
    // * height == byte_count`, so the render cannot write out of bounds.
    unsafe {
        context.render_toBitmap_rowBytes_bounds_format_colorSpace(
            &ci_image,
            NonNull::new(rgba16f.as_mut_ptr().cast())
                .expect("a non-empty Vec has a non-null data pointer"),
            bytes_per_row,
            CGRect::new(
                CGPoint::new(0.0, 0.0),
                CGSize::new(f64::from(width), f64::from(height)),
            ),
            kCIFormatRGBAh,
            Some(&color_space),
        );
    }

    if rgba16f_has_hdr_headroom(&rgba16f) {
        return Ok(AppleDecodedImage {
            width,
            height,
            pixels: rgba16f,
            pixel_format: AppleDecodedPixelFormat::Rgba16Float,
            hdr: true,
        });
    }

    Ok(AppleDecodedImage {
        width,
        height,
        pixels: rgba16f_to_rgba8(&rgba16f),
        pixel_format: AppleDecodedPixelFormat::Rgba8UnormSrgb,
        hdr: false,
    })
}

/// Whether any pixel's R, G or B channel exceeds the SDR headroom threshold.
fn rgba16f_has_hdr_headroom(rgba16f: &[u8]) -> bool {
    rgba16f.as_chunks::<8>().0.iter().any(|pixel| {
        pixel[..6]
            .as_chunks::<2>()
            .0
            .iter()
            .any(|channel| f16::from_le_bytes(*channel).to_f32() > HDR_HEADROOM_THRESHOLD)
    })
}

/// Quantizes a scaled value to `u8`; callers clamp into 0..=255 first.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to 0..=255 before the cast"
)]
const fn round_to_u8(value: f32) -> u8 {
    value.round() as u8
}

/// Clamps a linear value to [0, 1] and quantizes it to 8-bit.
fn linear_to_unorm8(value: f32) -> u8 {
    round_to_u8(value.clamp(0.0, 1.0) * 255.0)
}

/// Clamps a linear value to [0, 1], applies the sRGB transfer function and
/// quantizes it to 8-bit.
#[expect(
    clippy::suboptimal_flops,
    reason = "keeps the transfer-function arithmetic of the previous implementation bit-for-bit"
)]
fn linear_to_srgb_unorm8(value: f32) -> u8 {
    let clamped = value.clamp(0.0, 1.0);
    let encoded = if clamped <= 0.003_130_8 {
        clamped * 12.92
    } else {
        1.055 * clamped.powf(1.0 / 2.4) - 0.055
    };
    round_to_u8(encoded * 255.0)
}

/// Tonemaps the `RGBA16F` bitmap to sRGB `RGBA8`. `rgba16f`'s length is a
/// multiple of 8 by construction in `decode_isobmff_image`.
fn rgba16f_to_rgba8(rgba16f: &[u8]) -> Vec<u8> {
    let mut rgba8 = Vec::with_capacity(rgba16f.len() / 2);
    for pixel in rgba16f.as_chunks::<8>().0 {
        let channels = pixel.as_chunks::<2>().0;
        let r = f16::from_le_bytes(channels[0]).to_f32();
        let g = f16::from_le_bytes(channels[1]).to_f32();
        let b = f16::from_le_bytes(channels[2]).to_f32();
        let a = f16::from_le_bytes(channels[3]).to_f32();
        rgba8.extend_from_slice(&[
            linear_to_srgb_unorm8(r),
            linear_to_srgb_unorm8(g),
            linear_to_srgb_unorm8(b),
            linear_to_unorm8(a),
        ]);
    }
    rgba8
}
