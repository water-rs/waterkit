#[cfg(any(target_os = "android", target_vendor = "apple", test))]
use waterkit_video_core::{ColorRange, MatrixCoefficients};
#[cfg(any(target_os = "android", test))]
use wgpu_external_frame::YcbcrEncoding;
#[cfg(any(target_os = "android", target_vendor = "apple", test))]
use wgpu_external_frame::{YcbcrMatrix, YcbcrRange};

#[cfg(any(target_os = "android", target_vendor = "apple", test))]
pub const fn from_ycbcr_matrix(matrix: YcbcrMatrix) -> MatrixCoefficients {
    match matrix {
        YcbcrMatrix::Bt601 => MatrixCoefficients::Bt601,
        YcbcrMatrix::Bt709 => MatrixCoefficients::Bt709,
        YcbcrMatrix::Bt2020 => MatrixCoefficients::Bt2020NonConstantLuminance,
    }
}

#[cfg(any(target_os = "android", target_vendor = "apple", test))]
pub const fn from_ycbcr_range(range: YcbcrRange) -> ColorRange {
    match range {
        YcbcrRange::Video => ColorRange::Limited,
        YcbcrRange::Full => ColorRange::Full,
    }
}

#[cfg(any(target_os = "android", test))]
pub const fn from_ycbcr_encoding(encoding: YcbcrEncoding) -> (MatrixCoefficients, ColorRange) {
    (
        from_ycbcr_matrix(encoding.matrix),
        from_ycbcr_range(encoding.range),
    )
}

#[cfg(test)]
mod tests {
    use waterkit_video_core::{ColorRange, MatrixCoefficients};
    use wgpu_external_frame::{YcbcrEncoding, YcbcrMatrix, YcbcrRange};

    use super::from_ycbcr_encoding;

    #[test]
    fn maps_ycbcr_matrix_and_range() {
        for (source_matrix, matrix) in [
            (YcbcrMatrix::Bt601, MatrixCoefficients::Bt601),
            (YcbcrMatrix::Bt709, MatrixCoefficients::Bt709),
            (
                YcbcrMatrix::Bt2020,
                MatrixCoefficients::Bt2020NonConstantLuminance,
            ),
        ] {
            for (source_range, range) in [
                (YcbcrRange::Video, ColorRange::Limited),
                (YcbcrRange::Full, ColorRange::Full),
            ] {
                assert_eq!(
                    from_ycbcr_encoding(YcbcrEncoding {
                        matrix: source_matrix,
                        range: source_range,
                    }),
                    (matrix, range)
                );
            }
        }
    }
}
