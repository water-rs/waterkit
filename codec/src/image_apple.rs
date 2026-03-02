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
    }

    extern "Swift" {
        fn decode_heif_image(data: Vec<u8>) -> SwiftDecodedImage;
    }
}

#[derive(Debug)]
pub(crate) struct AppleDecodedImage {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) pixels: Vec<u8>,
    pub(crate) hdr: bool,
}

pub(crate) fn decode_heif_image_rgba8(data: &[u8]) -> Result<AppleDecodedImage, CodecError> {
    let decoded = ffi::decode_heif_image(data.to_vec());
    if !decoded.is_valid {
        return Err(CodecError::DecodingFailed(
            "Swift HEIF decode returned invalid image".into(),
        ));
    }
    if decoded.width == 0 || decoded.height == 0 {
        return Err(CodecError::DecodingFailed(
            "Swift HEIF decode returned zero-sized image".into(),
        ));
    }

    let expected_len = (decoded.width as usize)
        .checked_mul(decoded.height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| CodecError::DecodingFailed("HEIF decoded buffer size overflow".into()))?;
    if decoded.pixels.len() != expected_len {
        return Err(CodecError::DecodingFailed(format!(
            "Swift HEIF decode returned {} bytes, expected {}",
            decoded.pixels.len(),
            expected_len
        )));
    }

    Ok(AppleDecodedImage {
        width: decoded.width,
        height: decoded.height,
        pixels: decoded.pixels,
        hdr: decoded.hdr,
    })
}
