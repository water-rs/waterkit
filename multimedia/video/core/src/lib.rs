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

impl VideoColorInfo {
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

    use super::{ColorPrimaries, FrameRate, FrameTiming, TransferFunction, VideoColorInfo};

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
