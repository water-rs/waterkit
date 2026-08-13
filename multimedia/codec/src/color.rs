//! Shared color-conversion contract for decoded YUV textures.

use waterkit_video_core::{
    ColorPrimaries, ColorRange, MatrixCoefficients, TransferFunction, VideoColorInfo,
};

use crate::DecodedPixelLayout;

/// Unified GPU shader used by `WaterKit` conversion and presentation pipelines.
pub const YUV_COLOR_SHADER_WGSL: &str = include_str!("yuv_to_rgba.wgsl");

/// `WaterUI`'s linear-light value for diffuse SDR white, in nits.
pub const SDR_REFERENCE_WHITE_NITS: f32 = 203.0;

/// Color-space contract expected by the destination texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ColorOutputTarget {
    /// Gamma-encoded BT.709 output for a non-sRGB render target.
    GammaSdr = 0,
    /// Linear SDR output for an sRGB render target.
    LinearSdr = 1,
    /// Linear extended-range output preserving HDR relative to 203-nit white.
    LinearHdr = 2,
}

/// Shader uniform encoding source and destination color semantics.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct VideoColorUniform {
    matrix_mode: u32,
    range_mode: u32,
    primaries_mode: u32,
    transfer_mode: u32,
    target_mode: u32,
    sample_mode: u32,
    max_content_light_nits: f32,
    padding: u32,
}

impl VideoColorUniform {
    /// Serializes the uniform using its WGSL-compatible 32-byte layout.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        let mut bytes = [0_u8; 32];
        bytes[0..4].copy_from_slice(&self.matrix_mode.to_ne_bytes());
        bytes[4..8].copy_from_slice(&self.range_mode.to_ne_bytes());
        bytes[8..12].copy_from_slice(&self.primaries_mode.to_ne_bytes());
        bytes[12..16].copy_from_slice(&self.transfer_mode.to_ne_bytes());
        bytes[16..20].copy_from_slice(&self.target_mode.to_ne_bytes());
        bytes[20..24].copy_from_slice(&self.sample_mode.to_ne_bytes());
        bytes[24..28].copy_from_slice(&self.max_content_light_nits.to_ne_bytes());
        bytes[28..32].copy_from_slice(&self.padding.to_ne_bytes());
        bytes
    }
}

/// Builds a shader uniform for one decoded frame and destination target.
#[must_use]
pub fn video_color_uniform(
    color: VideoColorInfo,
    layout: DecodedPixelLayout,
    target: ColorOutputTarget,
) -> VideoColorUniform {
    VideoColorUniform {
        matrix_mode: match color.matrix {
            MatrixCoefficients::Bt709 => 0,
            MatrixCoefficients::Bt601 => 1,
            MatrixCoefficients::Bt2020NonConstantLuminance => 2,
            MatrixCoefficients::Bt2020ConstantLuminance => 3,
        },
        range_mode: match color.range {
            ColorRange::Limited => 0,
            ColorRange::Full => 1,
        },
        primaries_mode: match color.primaries {
            ColorPrimaries::Bt709 => 0,
            ColorPrimaries::Bt601 => 1,
            ColorPrimaries::DisplayP3 => 2,
            ColorPrimaries::Bt2020 => 3,
        },
        transfer_mode: match color.transfer {
            TransferFunction::Sdr => 0,
            TransferFunction::Pq => 1,
            TransferFunction::Hlg => 2,
        },
        target_mode: target as u32,
        sample_mode: match layout {
            DecodedPixelLayout::Nv12 => 0,
            DecodedPixelLayout::P010 => 1,
        },
        max_content_light_nits: color.content_light_level.map_or_else(
            || match color.transfer {
                TransferFunction::Sdr => SDR_REFERENCE_WHITE_NITS,
                TransferFunction::Pq => 10_000.0,
                TransferFunction::Hlg => 1_000.0,
            },
            |level| f32::from(level.max_content_light_level()),
        ),
        padding: 0,
    }
}

#[cfg(test)]
mod tests {
    use waterkit_video_core::{
        ColorPrimaries, ColorRange, MatrixCoefficients, TransferFunction, VideoColorInfo,
    };

    use super::{ColorOutputTarget, SDR_REFERENCE_WHITE_NITS, video_color_uniform};
    use crate::DecodedPixelLayout;

    #[test]
    fn p010_hdr_uniform_has_stable_wgsl_layout() {
        let uniform = video_color_uniform(
            VideoColorInfo {
                matrix: MatrixCoefficients::Bt2020NonConstantLuminance,
                primaries: ColorPrimaries::Bt2020,
                transfer: TransferFunction::Pq,
                range: ColorRange::Limited,
                content_light_level: None,
                dolby_vision: false,
            },
            DecodedPixelLayout::P010,
            ColorOutputTarget::LinearHdr,
        );
        let bytes = uniform.to_bytes();

        assert_eq!(bytes.len(), 32);
        assert_eq!(u32::from_ne_bytes(bytes[20..24].try_into().unwrap()), 1);
        assert!(
            (f32::from_ne_bytes(bytes[24..28].try_into().unwrap()) - 10_000.0).abs() < f32::EPSILON
        );
    }

    #[test]
    fn sdr_uniform_uses_framework_reference_white() {
        let uniform = video_color_uniform(
            VideoColorInfo::default(),
            DecodedPixelLayout::Nv12,
            ColorOutputTarget::LinearSdr,
        );
        let bytes = uniform.to_bytes();
        let reference_white = f32::from_ne_bytes(bytes[24..28].try_into().unwrap());
        assert!((reference_white - SDR_REFERENCE_WHITE_NITS).abs() < f32::EPSILON);
    }
}
