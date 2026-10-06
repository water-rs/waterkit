use waterkit_video_core::{
    ColorPrimaries, ColorRange, MatrixCoefficients, TransferFunction, VideoColorInfo,
};

pub fn bt709_sdr_limited() -> VideoColorInfo {
    VideoColorInfo {
        matrix: MatrixCoefficients::Bt709,
        primaries: ColorPrimaries::Bt709,
        transfer: TransferFunction::Sdr,
        range: ColorRange::Limited,
        content_light_level: None,
        dolby_vision: false,
    }
}
