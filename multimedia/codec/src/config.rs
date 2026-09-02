use crate::{CodecError, DecodedPixelLayout};

pub fn decoded_pixel_layout(
    is_hevc: bool,
    config: Option<&[u8]>,
) -> Result<DecodedPixelLayout, CodecError> {
    if !is_hevc {
        return Ok(DecodedPixelLayout::Nv12);
    }
    let config = config.ok_or_else(|| {
        CodecError::InitializationFailed(
            "HEVC codec configuration is required to select an output bit depth".into(),
        )
    })?;
    let payload = strip_box_header(config, *b"hvcC");
    if payload.len() < 19 {
        return Err(CodecError::InitializationFailed(
            "invalid hvcC payload: missing bit-depth fields".into(),
        ));
    }
    let luma_bit_depth = 8 + (payload[17] & 0x07);
    let chroma_bit_depth = 8 + (payload[18] & 0x07);
    match (luma_bit_depth, chroma_bit_depth) {
        (8, 8) => Ok(DecodedPixelLayout::Nv12),
        (10, 10) => Ok(DecodedPixelLayout::P010),
        _ => Err(CodecError::Unsupported(format!(
            "hardware decode supports 8-bit NV12 and 10-bit P010, not {luma_bit_depth}-bit luma/{chroma_bit_depth}-bit chroma"
        ))),
    }
}

pub fn strip_box_header(config: &[u8], box_type: [u8; 4]) -> &[u8] {
    if config.len() >= 8 && config[4..8] == box_type {
        &config[8..]
    } else {
        config
    }
}

#[cfg(test)]
mod tests {
    use super::decoded_pixel_layout;
    use crate::DecodedPixelLayout;

    #[test]
    fn reads_hevc_bit_depth_from_boxed_hvcc() {
        let mut config = vec![0_u8; 8 + 23];
        config[4..8].copy_from_slice(b"hvcC");
        config[8] = 1;
        config[8 + 17] = 2;
        config[8 + 18] = 2;
        assert_eq!(
            decoded_pixel_layout(true, Some(&config)).expect("valid Main10 configuration"),
            DecodedPixelLayout::P010
        );
    }
}
