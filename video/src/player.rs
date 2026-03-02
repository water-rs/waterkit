//! High-level video player with hardware-accelerated decode.
//!
//! Combines container demuxing with hardware video decode.

use crate::{VideoError, VideoReader};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use waterkit_codec::{CodecType, Decoder, GpuFrame};
use wgpu::{Device, Queue};

/// A decoded video frame ready for GPU rendering.
pub struct PlayerFrame {
    /// The GPU frame with Y and UV textures.
    pub gpu_frame: GpuFrame,
    /// Presentation timestamp.
    pub pts: Duration,
    /// Frame index.
    pub index: u64,
}

impl std::fmt::Debug for PlayerFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlayerFrame")
            .field("pts", &self.pts)
            .field("index", &self.index)
            .finish_non_exhaustive()
    }
}

/// Video player with hardware-accelerated decode.
///
/// Provides high-level playback API that handles:
/// - Container demuxing (MP4/MOV)
/// - Hardware video decode (via `waterkit-codec`)
/// - GPU texture creation
pub struct VideoPlayer {
    reader: VideoReader,
    decoder: Decoder,
    device: Arc<Device>,
    queue: Arc<Queue>,
    frame_index: u64,
    codec_type: CodecType,
}

impl std::fmt::Debug for VideoPlayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoPlayer")
            .field("dimensions", &self.reader.dimensions())
            .field("frame_index", &self.frame_index)
            .finish_non_exhaustive()
    }
}

impl VideoPlayer {
    /// Open a video file for playback.
    ///
    /// # Arguments
    /// * `path` - Path to the video file (MP4 or MOV)
    /// * `device` - wgpu device for GPU texture creation
    /// * `queue` - wgpu queue for GPU operations
    ///
    /// # Errors
    /// Returns error if the file cannot be opened or the codec is not supported.
    pub fn open<P: AsRef<Path>>(
        path: P,
        device: Arc<Device>,
        queue: Arc<Queue>,
    ) -> Result<Self, VideoError> {
        let reader = VideoReader::open(path)?;
        let (width, height) = reader.dimensions();

        // Detect codec type from codec config
        let codec_config = reader.codec_config();
        let codec_type = detect_codec_type(codec_config)?;

        // Create hardware decoder
        let decoder = Decoder::new(codec_type, codec_config, width, height)
            .map_err(|e| VideoError::Codec(e.to_string()))?;

        Ok(Self {
            reader,
            decoder,
            device,
            queue,
            frame_index: 0,
            codec_type,
        })
    }

    /// Get video dimensions (width, height).
    #[must_use]
    pub const fn dimensions(&self) -> (u32, u32) {
        self.reader.dimensions()
    }

    /// Get the codec type.
    #[must_use]
    pub const fn codec_type(&self) -> CodecType {
        self.codec_type
    }

    /// Get the total number of samples (frames).
    #[must_use]
    pub const fn sample_count(&self) -> u32 {
        self.reader.sample_count()
    }

    /// Get the timescale of the video.
    #[must_use]
    pub const fn timescale(&self) -> u32 {
        self.reader.timescale()
    }

    /// Decode the next frame.
    ///
    /// Returns `None` when there are no more frames.
    ///
    /// # Errors
    /// Returns error if decoding fails.
    pub fn next_frame(&mut self) -> Result<Option<PlayerFrame>, VideoError> {
        let Some((sample_data, pts, _is_keyframe)) = self.reader.read_sample() else {
            return Ok(None);
        };

        // Decode the sample
        let mut decode_stream = self.decoder.decode(&sample_data);

        // Get the first decoded frame (there may be multiple due to B-frames)
        let Some(result) = decode_stream.next() else {
            return Ok(None);
        };

        let decoded = result.map_err(|e| VideoError::Codec(e.to_string()))?;

        // Convert to GPU frame
        let gpu_frame = decoded.to_gpu_frame(&self.device, &self.queue);

        // Calculate PTS in Duration
        let timescale = self.reader.timescale();
        let pts_duration = if timescale > 0 {
            Duration::from_nanos((pts * 1_000_000_000) / u64::from(timescale))
        } else {
            Duration::ZERO
        };

        let frame = PlayerFrame {
            gpu_frame,
            pts: pts_duration,
            index: self.frame_index,
        };

        self.frame_index += 1;

        Ok(Some(frame))
    }

    /// Create an async stream of decoded frames.
    ///
    /// Frames are decoded on demand as the stream is consumed.
    pub fn frames(self) -> impl futures::Stream<Item = Result<PlayerFrame, VideoError>> {
        futures::stream::unfold(self, move |mut player| async move {
            match player.next_frame() {
                Ok(Some(frame)) => Some((Ok(frame), player)),
                Ok(None) => None,
                Err(e) => Some((Err(e), player)),
            }
        })
    }

    /// Reset playback to the beginning.
    pub const fn reset(&mut self) {
        self.reader.reset();
        self.frame_index = 0;
    }
}

/// Detect codec type from codec configuration data.
fn detect_codec_type(config: Option<&[u8]>) -> Result<CodecType, VideoError> {
    let config = config.ok_or_else(|| VideoError::Unsupported("No codec config found".into()))?;

    // Check for avcC (H.264) - starts with version byte 0x01
    // The structure is: version(1) + profile(1) + compatibility(1) + level(1) + ...
    if config.len() >= 8 && &config[4..8] == b"avcC" {
        return Ok(CodecType::H264);
    }

    // Check for raw avcC without box header
    if config.len() >= 7 && config[0] == 0x01 {
        // Could be either avcC or hvcC, need more checks
        // avcC has profile at offset 1, hvcC has different structure
        // Check for AVCConfigurationVersion == 1 and valid profile_idc
        let profile = config[1];
        if matches!(profile, 66 | 77 | 88 | 100 | 110 | 122 | 144) {
            return Ok(CodecType::H264);
        }
    }

    // Check for hvcC (H.265) - look for box header
    if config.len() >= 8 && &config[4..8] == b"hvcC" {
        return Ok(CodecType::H265);
    }

    // Check for raw hvcC without box header
    // hvcC starts with: configurationVersion(1) + general_profile_space/tier/profile(1) + ...
    if config.len() >= 23 && config[0] == 0x01 {
        // Check if it looks like hvcC structure
        // general_profile_compatibility_flags at offset 2-5
        // general_constraint_indicator_flags at offset 6-11
        // general_level_idc at offset 12
        let level = config[12];
        if level > 0 && level <= 186 {
            // Valid HEVC levels are up to 6.2 (186 / 30)
            return Ok(CodecType::H265);
        }
    }

    Err(VideoError::Unsupported(
        "Unknown codec (only H.264 and H.265 are supported)".into(),
    ))
}
