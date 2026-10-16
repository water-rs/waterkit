//! Engine-neutral types shared by the `WaterKit` video crate family.
//!
//! This crate intentionally contains no container parser, codec, network,
//! graphics, audio-output, or UI dependency. It is suitable for applications,
//! media tools, playback engines, and processing libraries that need to agree
//! on media timing and color semantics without importing an implementation.

#![warn(missing_docs)]

mod protection;

pub use protection::{
    CommonEncryptionScheme, EncryptionSubsample, ProtectionInitData, SampleEncryption,
    TrackProtection,
};

use std::{num::NonZeroU32, time::Duration};

/// Declares the mode codes [`YCBCR_WGSL`]'s functions take, once, as Rust
/// constants in [`ycbcr_mode`] and as the WGSL constants the fragment opens
/// with, so a converter's uniform and its shader cannot disagree.
macro_rules! ycbcr_modes {
    ($($(#[$doc:meta])* $name:ident = $value:literal,)*) => {
        /// The `u32` codes [`YCBCR_WGSL`]'s `matrix_mode` and `range_mode`
        /// parameters take. Each is also declared in WGSL as `YCBCR_<name>`.
        pub mod ycbcr_mode {
            $($(#[$doc])* pub const $name: u32 = $value;)*
        }

        /// WGSL fragment holding the YCbCr to RGB range and matrix math that
        /// every `WaterKit` GPU colour converter shares.
        ///
        /// It declares the [`ycbcr_mode`] constants and the conversion
        /// functions, but no bindings. A converter prepends it to its own
        /// shader source before compiling, so the decoder's YUV path and the
        /// camera's frame converter evaluate the same coefficients.
        pub const YCBCR_WGSL: &str = concat!(
            $("const YCBCR_", stringify!($name), ": u32 = ", stringify!($value), "u;\n",)*
            include_str!("ycbcr.wgsl"),
        );
    };
}

ycbcr_modes! {
    /// ITU-R BT.709 matrix.
    MATRIX_BT709 = 0,
    /// ITU-R BT.601 matrix.
    MATRIX_BT601 = 1,
    /// ITU-R BT.2020 non-constant-luminance matrix.
    MATRIX_BT2020 = 2,
    /// Video ("limited") range.
    RANGE_LIMITED = 0,
    /// Full range.
    RANGE_FULL = 1,
}

/// Error returned by `WaterKit` video operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An operating-system I/O operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// A media container is malformed or unsupported.
    #[error("container error: {0}")]
    Container(String),

    /// An encoder or decoder failed.
    #[error("codec error: {0}")]
    Codec(String),

    /// A network or streaming operation failed.
    #[error("streaming error: {0}")]
    Streaming(String),

    /// A media-processing operation failed.
    #[error("processing error: {0}")]
    Processing(String),

    /// A platform media service failed.
    #[error("platform media error: {0}")]
    Platform(String),

    /// The requested capability is unavailable for the supplied media or platform.
    #[error("unsupported capability: {0}")]
    Unsupported(String),
}

/// Presentation timing attached to one decoded or processed frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameTiming {
    presentation_time: Duration,
    duration: Duration,
    sequence: u64,
    discontinuity: bool,
}

impl FrameTiming {
    /// Creates deterministic timing for one frame.
    #[must_use]
    pub const fn new(presentation_time: Duration, duration: Duration, sequence: u64) -> Self {
        Self {
            presentation_time,
            duration,
            sequence,
            discontinuity: false,
        }
    }

    /// Marks whether this frame starts a discontinuous media-time segment.
    #[must_use]
    pub const fn with_discontinuity(mut self, discontinuity: bool) -> Self {
        self.discontinuity = discontinuity;
        self
    }

    /// Returns the presentation timestamp on the media timeline.
    #[must_use]
    pub const fn presentation_time(self) -> Duration {
        self.presentation_time
    }

    /// Returns the expected display duration of this frame.
    #[must_use]
    pub const fn duration(self) -> Duration {
        self.duration
    }

    /// Returns the monotonically increasing frame sequence number.
    #[must_use]
    pub const fn sequence(self) -> u64 {
        self.sequence
    }

    /// Returns whether this frame starts a discontinuous segment.
    #[must_use]
    pub const fn is_discontinuity(self) -> bool {
        self.discontinuity
    }
}

/// Exact rational frame rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameRate {
    numerator: NonZeroU32,
    denominator: NonZeroU32,
}

impl FrameRate {
    /// Creates an exact rational frame rate.
    #[must_use]
    pub const fn new(numerator: NonZeroU32, denominator: NonZeroU32) -> Self {
        Self {
            numerator,
            denominator,
        }
    }

    /// Returns the rate numerator.
    #[must_use]
    pub const fn numerator(self) -> NonZeroU32 {
        self.numerator
    }

    /// Returns the rate denominator.
    #[must_use]
    pub const fn denominator(self) -> NonZeroU32 {
        self.denominator
    }

    /// Returns the frame rate as frames per second.
    #[must_use]
    pub fn as_f64(self) -> f64 {
        f64::from(self.numerator.get()) / f64::from(self.denominator.get())
    }
}

/// Non-zero coded video dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameSize {
    width: NonZeroU32,
    height: NonZeroU32,
}

impl FrameSize {
    /// Creates non-zero coded dimensions.
    #[must_use]
    pub const fn new(width: NonZeroU32, height: NonZeroU32) -> Self {
        Self { width, height }
    }

    /// Returns the coded width in pixels.
    #[must_use]
    pub const fn width(self) -> NonZeroU32 {
        self.width
    }

    /// Returns the coded height in pixels.
    #[must_use]
    pub const fn height(self) -> NonZeroU32 {
        self.height
    }
}

/// YUV-to-RGB matrix coefficients signaled by a video stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MatrixCoefficients {
    /// ITU-R BT.601 matrix coefficients.
    Bt601,
    /// ITU-R BT.709 matrix coefficients.
    #[default]
    Bt709,
    /// Constant-luminance ITU-R BT.2020 matrix coefficients.
    Bt2020ConstantLuminance,
    /// Non-constant-luminance ITU-R BT.2020 matrix coefficients.
    Bt2020NonConstantLuminance,
}

impl MatrixCoefficients {
    /// Returns the canonical ITU-T H.273 matrix-coefficients code point.
    #[must_use]
    pub const fn cicp(self) -> u8 {
        match self {
            Self::Bt709 => 1,
            Self::Bt601 => 6,
            Self::Bt2020NonConstantLuminance => 9,
            Self::Bt2020ConstantLuminance => 10,
        }
    }

    /// Returns the matrix coefficients represented by a known H.273 code point.
    ///
    /// Both code points 5 (ITU-R BT.470 System B, G) and 6 (SMPTE 170M)
    /// use the BT.601 matrix in this API.
    #[must_use]
    pub const fn from_cicp(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Bt709),
            5 | 6 => Some(Self::Bt601),
            9 => Some(Self::Bt2020NonConstantLuminance),
            10 => Some(Self::Bt2020ConstantLuminance),
            _ => None,
        }
    }

    /// The [`ycbcr_mode`] matrix code [`YCBCR_WGSL`] decodes these
    /// coefficients with, or `None` for constant-luminance BT.2020, which is
    /// not a matrix and which the shared fragment does not implement.
    #[must_use]
    pub const fn ycbcr_mode(self) -> Option<u32> {
        match self {
            Self::Bt709 => Some(ycbcr_mode::MATRIX_BT709),
            Self::Bt601 => Some(ycbcr_mode::MATRIX_BT601),
            Self::Bt2020NonConstantLuminance => Some(ycbcr_mode::MATRIX_BT2020),
            Self::Bt2020ConstantLuminance => None,
        }
    }
}

/// Color primaries signaled by a video stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ColorPrimaries {
    /// ITU-R BT.601 primaries.
    Bt601,
    /// ITU-R BT.709 primaries.
    #[default]
    Bt709,
    /// Display P3 primaries.
    DisplayP3,
    /// ITU-R BT.2020 primaries.
    Bt2020,
}

/// Electro-optical transfer function signaled by a video stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TransferFunction {
    /// Conventional standard-dynamic-range transfer function.
    ///
    /// This does not distinguish BT.709, sRGB, or SMPTE 170M; CICP signaling
    /// uses code point 1 (BT.709) for this value.
    #[default]
    Sdr,
    /// SMPTE ST 2084 perceptual quantizer.
    Pq,
    /// ARIB STD-B67 hybrid log-gamma.
    Hlg,
}

/// Encoded component range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ColorRange {
    /// Studio/video range.
    #[default]
    Limited,
    /// Full component range.
    Full,
}

impl ColorRange {
    /// The [`ycbcr_mode`] range code [`YCBCR_WGSL`] decodes this range with.
    #[must_use]
    pub const fn ycbcr_mode(self) -> u32 {
        match self {
            Self::Limited => ycbcr_mode::RANGE_LIMITED,
            Self::Full => ycbcr_mode::RANGE_FULL,
        }
    }
}

/// Static content-light metadata for HDR video.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentLightLevel {
    max_content_light_level: u16,
    max_frame_average_light_level: u16,
}

impl ContentLightLevel {
    /// Creates CTA-861 content-light metadata, expressed in nits.
    #[must_use]
    pub const fn new(max_content_light_level: u16, max_frame_average_light_level: u16) -> Self {
        Self {
            max_content_light_level,
            max_frame_average_light_level,
        }
    }

    /// Returns `MaxCLL` in nits.
    #[must_use]
    pub const fn max_content_light_level(self) -> u16 {
        self.max_content_light_level
    }

    /// Returns `MaxFALL` in nits.
    #[must_use]
    pub const fn max_frame_average_light_level(self) -> u16 {
        self.max_frame_average_light_level
    }
}

/// Color description that travels with decoded and processed video frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct VideoColorInfo {
    /// Matrix coefficients used by encoded YUV samples.
    pub matrix: MatrixCoefficients,
    /// Source color primaries.
    pub primaries: ColorPrimaries,
    /// Source transfer function.
    pub transfer: TransferFunction,
    /// Encoded component range.
    pub range: ColorRange,
    /// Optional static content-light metadata.
    pub content_light_level: Option<ContentLightLevel>,
    /// Whether Dolby Vision configuration was signaled.
    pub dolby_vision: bool,
}

/// ITU-T H.273 (CICP) code points, as video bitstreams and the ISO BMFF
/// `colr`/`nclx` box carry them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CicpColor {
    /// Color-primaries code point: BT.709 1, BT.601/SMPTE 170M 6,
    /// BT.2020 9, or Display P3 12.
    pub primaries: u8,
    /// Transfer-characteristics code point: SDR/BT.709 1, PQ 16, or HLG 18.
    pub transfer: u8,
    /// Matrix-coefficients code point: BT.709 1, BT.601 6,
    /// BT.2020 non-constant-luminance 9, or BT.2020 constant-luminance 10.
    pub matrix: u8,
    /// Whether the encoded component values use full range.
    pub full_range: bool,
}

impl VideoColorInfo {
    /// Returns this description's ITU-T H.273 (CICP) code points.
    ///
    /// `TransferFunction::Sdr` maps to code point 1 (BT.709), because the
    /// shared SDR value does not distinguish BT.709, sRGB, and SMPTE 170M.
    #[must_use]
    pub const fn cicp(&self) -> CicpColor {
        CicpColor {
            primaries: match self.primaries {
                ColorPrimaries::Bt709 => 1,
                ColorPrimaries::Bt601 => 6,
                ColorPrimaries::Bt2020 => 9,
                ColorPrimaries::DisplayP3 => 12,
            },
            transfer: match self.transfer {
                TransferFunction::Sdr => 1,
                TransferFunction::Pq => 16,
                TransferFunction::Hlg => 18,
            },
            matrix: self.matrix.cicp(),
            full_range: matches!(self.range, ColorRange::Full),
        }
    }

    /// Returns whether this description represents HDR transfer characteristics.
    #[must_use]
    pub const fn is_hdr(self) -> bool {
        matches!(self.transfer, TransferFunction::Pq | TransferFunction::Hlg)
    }

    /// Returns whether this description uses wide-gamut primaries.
    #[must_use]
    pub const fn is_wide_gamut(self) -> bool {
        matches!(
            self.primaries,
            ColorPrimaries::DisplayP3 | ColorPrimaries::Bt2020
        )
    }
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroU32, time::Duration};

    use super::{
        CicpColor, ColorPrimaries, ColorRange, FrameRate, FrameTiming, MatrixCoefficients,
        TransferFunction, VideoColorInfo,
    };

    #[test]
    fn video_color_info_maps_every_supported_cicp_code_point() {
        let mut color = VideoColorInfo::default();

        for (primaries, code) in [
            (ColorPrimaries::Bt709, 1),
            (ColorPrimaries::Bt601, 6),
            (ColorPrimaries::Bt2020, 9),
            (ColorPrimaries::DisplayP3, 12),
        ] {
            color.primaries = primaries;
            assert_eq!(color.cicp().primaries, code);
        }

        for (transfer, code) in [
            (TransferFunction::Sdr, 1),
            (TransferFunction::Pq, 16),
            (TransferFunction::Hlg, 18),
        ] {
            color.transfer = transfer;
            assert_eq!(color.cicp().transfer, code);
        }

        for (matrix, code) in [
            (MatrixCoefficients::Bt709, 1),
            (MatrixCoefficients::Bt601, 6),
            (MatrixCoefficients::Bt2020NonConstantLuminance, 9),
            (MatrixCoefficients::Bt2020ConstantLuminance, 10),
        ] {
            color.matrix = matrix;
            assert_eq!(color.cicp().matrix, code);
        }

        color.range = ColorRange::Limited;
        assert!(!color.cicp().full_range);
        color.range = ColorRange::Full;
        assert!(color.cicp().full_range);
    }

    #[test]
    fn cicp_color_is_a_copyable_code_point_record() {
        let color = CicpColor {
            primaries: 9,
            transfer: 18,
            matrix: 9,
            full_range: false,
        };
        let copied_color = color;
        assert_eq!(color, copied_color);
    }

    #[test]
    fn matrix_coefficients_map_to_and_from_cicp() {
        for (matrix, code) in [
            (MatrixCoefficients::Bt709, 1),
            (MatrixCoefficients::Bt601, 6),
            (MatrixCoefficients::Bt2020NonConstantLuminance, 9),
            (MatrixCoefficients::Bt2020ConstantLuminance, 10),
        ] {
            assert_eq!(matrix.cicp(), code);
            assert_eq!(MatrixCoefficients::from_cicp(code), Some(matrix));
        }
        assert_eq!(
            MatrixCoefficients::from_cicp(5),
            Some(MatrixCoefficients::Bt601)
        );
        assert_eq!(MatrixCoefficients::from_cicp(0), None);
        assert_eq!(MatrixCoefficients::from_cicp(2), None);
    }

    #[test]
    fn frame_timing_retains_media_time_instead_of_wall_clock_time() {
        let timing = FrameTiming::new(Duration::from_secs(12), Duration::from_millis(40), 300)
            .with_discontinuity(true);

        assert_eq!(timing.presentation_time(), Duration::from_secs(12));
        assert_eq!(timing.duration(), Duration::from_millis(40));
        assert_eq!(timing.sequence(), 300);
        assert!(timing.is_discontinuity());
    }

    #[test]
    fn rational_frame_rate_preserves_broadcast_rates() {
        let rate = FrameRate::new(
            NonZeroU32::new(60_000).expect("rate numerator must be non-zero"),
            NonZeroU32::new(1_001).expect("rate denominator must be non-zero"),
        );
        assert!((rate.as_f64() - 59.940_059_940_059_94).abs() <= f64::EPSILON);
    }

    #[test]
    fn hdr_and_wide_gamut_are_derived_from_explicit_color_signals() {
        let color = VideoColorInfo {
            primaries: ColorPrimaries::Bt2020,
            transfer: TransferFunction::Pq,
            ..VideoColorInfo::default()
        };
        assert!(color.is_hdr());
        assert!(color.is_wide_gamut());
    }
}
