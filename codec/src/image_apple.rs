use crate::CodecError;

#[swift_bridge::bridge]
mod ffi {
    #[swift_bridge(swift_repr = "struct")]
    struct SwiftDecodedImage {
        width: u32,
        height: u32,
        pixels: Vec<u8>,
        is_valid: bool,
        hdr: bool,
        pixel_format: u8,
    }

    extern "Swift" {
        fn decode_isobmff_image(data: Vec<u8>) -> SwiftDecodedImage;
        fn av1_hardware_decode_supported() -> bool;
    }
}

const PIXEL_FORMAT_RGBA8_UNORM_SRGB: u8 = 1;
const PIXEL_FORMAT_RGBA16_FLOAT: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppleDecodedPixelFormat {
    Rgba8UnormSrgb,
    Rgba16Float,
}

#[derive(Debug)]
pub(crate) struct AppleDecodedImage {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) pixels: Vec<u8>,
    pub(crate) pixel_format: AppleDecodedPixelFormat,
    pub(crate) hdr: bool,
}

pub(crate) fn is_av1_hardware_decode_supported() -> bool {
    ffi::av1_hardware_decode_supported()
}

pub(crate) fn decode_isobmff_image(data: &[u8]) -> Result<AppleDecodedImage, CodecError> {
    let decoded = ffi::decode_isobmff_image(data.to_vec());
    if !decoded.is_valid {
        return Err(CodecError::DecodingFailed(
            "Swift ISOBMFF decode returned invalid image".into(),
        ));
    }
    if decoded.width == 0 || decoded.height == 0 {
        return Err(CodecError::DecodingFailed(
            "Swift ISOBMFF decode returned zero-sized image".into(),
        ));
    }

    let (pixel_format, bytes_per_pixel) = match decoded.pixel_format {
        PIXEL_FORMAT_RGBA8_UNORM_SRGB => (AppleDecodedPixelFormat::Rgba8UnormSrgb, 4usize),
        PIXEL_FORMAT_RGBA16_FLOAT => (AppleDecodedPixelFormat::Rgba16Float, 8usize),
        other => {
            return Err(CodecError::DecodingFailed(format!(
                "Swift ISOBMFF decode returned unknown pixel format tag {other}"
            )));
        }
    };

    let expected_len = (decoded.width as usize)
        .checked_mul(decoded.height as usize)
        .and_then(|pixels| pixels.checked_mul(bytes_per_pixel))
        .ok_or_else(|| CodecError::DecodingFailed("ISOBMFF decoded buffer size overflow".into()))?;
    if decoded.pixels.len() != expected_len {
        return Err(CodecError::DecodingFailed(format!(
            "Swift ISOBMFF decode returned {} bytes, expected {}",
            decoded.pixels.len(),
            expected_len
        )));
    }

    Ok(AppleDecodedImage {
        width: decoded.width,
        height: decoded.height,
        pixels: decoded.pixels,
        pixel_format,
        hdr: decoded.hdr,
    })
}
