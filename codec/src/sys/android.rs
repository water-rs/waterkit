//! Android `MediaCodec` hardware encoding and decoding.

use crate::CodecError;
use media_codec::{MediaCodec, MediaCodecDirection, MediaFormat};
use std::fmt;
use std::time::Duration;

/// Internal codec type for Android implementations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecType {
    H264,
    H265,
}

impl CodecType {
    fn mime_type(self) -> &'static str {
        match self {
            Self::H264 => "video/avc",
            Self::H265 => "video/hevc",
        }
    }
}

/// Android `MediaCodec` hardware decoder.
pub struct AndroidDecoder {
    codec: MediaCodec,
    codec_type: CodecType,
    width: u32,
    height: u32,
}

impl fmt::Debug for AndroidDecoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AndroidDecoder")
            .field("codec_type", &self.codec_type)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

unsafe impl Send for AndroidDecoder {}
unsafe impl Sync for AndroidDecoder {}

/// Decoded frame from Android MediaCodec (NV12 format).
#[derive(Clone)]
pub struct AndroidFrame {
    /// NV12 data: Y plane followed by interleaved UV plane.
    pub data: Vec<u8>,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Presentation timestamp in nanoseconds.
    pub timestamp_ns: u64,
}

impl fmt::Debug for AndroidFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AndroidFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("timestamp_ns", &self.timestamp_ns)
            .finish_non_exhaustive()
    }
}

impl AndroidDecoder {
    /// Create a new Android hardware decoder.
    ///
    /// # Arguments
    /// * `codec` - The codec type (H264 or H265)
    /// * `config` - Codec configuration data (avcC for H264, hvcC for H265). Can be None.
    /// * `width` - Video width in pixels
    /// * `height` - Video height in pixels
    pub fn new(
        codec_type: CodecType,
        config: Option<&[u8]>,
        width: u32,
        height: u32,
    ) -> Result<Self, CodecError> {
        let mime_type = codec_type.mime_type();

        // Create MediaCodec decoder
        let codec = MediaCodec::from_decoder_type(mime_type)
            .map_err(|e| CodecError::InitializationFailed(format!("MediaCodec create: {e}")))?;

        // Create format with dimensions
        let mut format = MediaFormat::new();
        format.set_str("mime", mime_type);
        format.set_i32("width", width as i32);
        format.set_i32("height", height as i32);
        format.set_i32("color-format", 0x15); // COLOR_FormatYUV420SemiPlanar (NV12)

        // Set codec-specific data (csd-0) if provided
        if let Some(config_data) = config {
            // Parse and set parameter sets
            match codec_type {
                CodecType::H264 => {
                    // avcC format - extract SPS/PPS
                    if let Some((sps, pps)) = parse_avcc(config_data) {
                        format.set_buffer("csd-0", &sps);
                        format.set_buffer("csd-1", &pps);
                    }
                }
                CodecType::H265 => {
                    // hvcC format - extract VPS/SPS/PPS
                    if let Some(csd) = parse_hvcc(config_data) {
                        format.set_buffer("csd-0", &csd);
                    }
                }
            }
        }

        // Configure the codec
        codec
            .configure(
                &format,
                None, // No surface for raw output
                MediaCodecDirection::Decoder,
            )
            .map_err(|e| CodecError::InitializationFailed(format!("MediaCodec configure: {e}")))?;

        // Start the codec
        codec
            .start()
            .map_err(|e| CodecError::InitializationFailed(format!("MediaCodec start: {e}")))?;

        Ok(Self {
            codec,
            codec_type,
            width,
            height,
        })
    }

    /// Decode compressed video data.
    pub fn decode(&mut self, data: &[u8]) -> Result<Vec<AndroidFrame>, CodecError> {
        // Get input buffer
        let input_index = self
            .codec
            .dequeue_input_buffer(Duration::from_millis(100))
            .map_err(|e| CodecError::DecodingFailed(format!("dequeue_input_buffer: {e}")))?;

        let Some(input_index) = input_index else {
            return Ok(Vec::new()); // No buffer available, try again later
        };

        // Get the input buffer and copy data
        let input_buffer = self
            .codec
            .input_buffer(input_index)
            .map_err(|e| CodecError::DecodingFailed(format!("input_buffer: {e}")))?;

        let copy_len = data.len().min(input_buffer.len());
        input_buffer[..copy_len].copy_from_slice(&data[..copy_len]);

        // Queue the input buffer
        self.codec
            .queue_input_buffer(input_index, 0, copy_len, 0, 0)
            .map_err(|e| CodecError::DecodingFailed(format!("queue_input_buffer: {e}")))?;

        // Collect output frames
        let mut frames = Vec::new();

        loop {
            let output_result = self
                .codec
                .dequeue_output_buffer(Duration::from_millis(10))
                .map_err(|e| CodecError::DecodingFailed(format!("dequeue_output_buffer: {e}")))?;

            match output_result {
                Some(output_info) => {
                    let output_buffer = self
                        .codec
                        .output_buffer(output_info.buffer_index())
                        .map_err(|e| CodecError::DecodingFailed(format!("output_buffer: {e}")))?;

                    // Extract NV12 data
                    let y_size = (self.width * self.height) as usize;
                    let uv_size = y_size / 2;
                    let frame_size = y_size + uv_size;

                    let offset = output_info.offset() as usize;
                    let size = output_info.size() as usize;
                    let end = (offset + size).min(output_buffer.len());
                    let actual_size = end - offset;

                    let mut nv12_data = vec![0u8; frame_size];
                    let copy_size = actual_size.min(frame_size);
                    nv12_data[..copy_size].copy_from_slice(&output_buffer[offset..offset + copy_size]);

                    let frame = AndroidFrame {
                        data: nv12_data,
                        width: self.width,
                        height: self.height,
                        timestamp_ns: output_info.presentation_time_us() as u64 * 1000,
                    };

                    frames.push(frame);

                    // Release the output buffer
                    self.codec
                        .release_output_buffer(output_info.buffer_index(), false)
                        .map_err(|e| {
                            CodecError::DecodingFailed(format!("release_output_buffer: {e}"))
                        })?;
                }
                None => break, // No more output available
            }
        }

        Ok(frames)
    }
}

impl Drop for AndroidDecoder {
    fn drop(&mut self) {
        let _ = self.codec.stop();
    }
}

/// Android `MediaCodec` hardware encoder.
pub struct AndroidEncoder {
    codec: MediaCodec,
    codec_type: CodecType,
    width: u32,
    height: u32,
    frame_count: i64,
    codec_config: Option<Vec<u8>>,
}

impl fmt::Debug for AndroidEncoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AndroidEncoder")
            .field("codec_type", &self.codec_type)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("frame_count", &self.frame_count)
            .finish_non_exhaustive()
    }
}

unsafe impl Send for AndroidEncoder {}
unsafe impl Sync for AndroidEncoder {}

impl AndroidEncoder {
    /// Create a new Android hardware encoder.
    pub fn new(codec_type: CodecType, width: u32, height: u32) -> Result<Self, CodecError> {
        let mime_type = codec_type.mime_type();

        // Create MediaCodec encoder
        let codec = MediaCodec::from_encoder_type(mime_type)
            .map_err(|e| CodecError::InitializationFailed(format!("MediaCodec create: {e}")))?;

        // Create format
        let mut format = MediaFormat::new();
        format.set_str("mime", mime_type);
        format.set_i32("width", width as i32);
        format.set_i32("height", height as i32);
        format.set_i32("color-format", 0x15); // COLOR_FormatYUV420SemiPlanar (NV12)
        format.set_i32("bitrate", 4_000_000); // 4 Mbps
        format.set_i32("frame-rate", 30);
        format.set_i32("i-frame-interval", 1); // Keyframe every 1 second

        // Configure the codec
        codec
            .configure(&format, None, MediaCodecDirection::Encoder)
            .map_err(|e| CodecError::InitializationFailed(format!("MediaCodec configure: {e}")))?;

        // Start the codec
        codec
            .start()
            .map_err(|e| CodecError::InitializationFailed(format!("MediaCodec start: {e}")))?;

        Ok(Self {
            codec,
            codec_type,
            width,
            height,
            frame_count: 0,
            codec_config: None,
        })
    }

    /// Encode NV12 data to compressed video.
    pub fn encode_nv12(&mut self, nv12: &[u8]) -> Result<Vec<u8>, CodecError> {
        let y_size = (self.width * self.height) as usize;
        let uv_size = y_size / 2;
        let expected_size = y_size + uv_size;

        if nv12.len() != expected_size {
            return Err(CodecError::EncodingFailed(format!(
                "NV12 data size {} doesn't match expected {} for {}x{}",
                nv12.len(),
                expected_size,
                self.width,
                self.height
            )));
        }

        // Get input buffer
        let input_index = self
            .codec
            .dequeue_input_buffer(Duration::from_millis(100))
            .map_err(|e| CodecError::EncodingFailed(format!("dequeue_input_buffer: {e}")))?;

        let Some(input_index) = input_index else {
            return Err(CodecError::EncodingFailed(
                "No input buffer available".into(),
            ));
        };

        // Copy NV12 data to input buffer
        let input_buffer = self
            .codec
            .input_buffer(input_index)
            .map_err(|e| CodecError::EncodingFailed(format!("input_buffer: {e}")))?;

        input_buffer[..expected_size].copy_from_slice(nv12);

        // Queue input buffer with timestamp
        let presentation_time_us = self.frame_count * 33333; // ~30fps
        self.frame_count += 1;

        self.codec
            .queue_input_buffer(input_index, 0, expected_size, presentation_time_us, 0)
            .map_err(|e| CodecError::EncodingFailed(format!("queue_input_buffer: {e}")))?;

        // Collect encoded output
        let mut encoded_data = Vec::new();

        loop {
            let output_result = self
                .codec
                .dequeue_output_buffer(Duration::from_millis(10))
                .map_err(|e| CodecError::EncodingFailed(format!("dequeue_output_buffer: {e}")))?;

            match output_result {
                Some(output_info) => {
                    let output_buffer = self
                        .codec
                        .output_buffer(output_info.buffer_index())
                        .map_err(|e| CodecError::EncodingFailed(format!("output_buffer: {e}")))?;

                    let offset = output_info.offset() as usize;
                    let size = output_info.size() as usize;

                    // Check if this is codec config (SPS/PPS)
                    let flags = output_info.flags();
                    if flags & 2 != 0 {
                        // BUFFER_FLAG_CODEC_CONFIG
                        self.codec_config = Some(output_buffer[offset..offset + size].to_vec());
                    } else {
                        encoded_data.extend_from_slice(&output_buffer[offset..offset + size]);
                    }

                    self.codec
                        .release_output_buffer(output_info.buffer_index(), false)
                        .map_err(|e| {
                            CodecError::EncodingFailed(format!("release_output_buffer: {e}"))
                        })?;
                }
                None => break,
            }
        }

        Ok(encoded_data)
    }

    /// Get the codec configuration data (avcC/hvcC) if available.
    #[must_use]
    pub fn get_codec_config(&self) -> Option<Vec<u8>> {
        self.codec_config.clone()
    }
}

impl Drop for AndroidEncoder {
    fn drop(&mut self) {
        let _ = self.codec.stop();
    }
}

/// Parse avcC (H.264 codec configuration) to extract SPS and PPS as Annex B format.
fn parse_avcc(data: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    if data.len() < 8 {
        return None;
    }

    // Check if it starts with a box header containing "avcC"
    let offset = if data.len() > 8 && &data[4..8] == b"avcC" {
        8
    } else {
        0
    };

    let data = &data[offset..];
    if data.len() < 7 {
        return None;
    }

    // avcC structure:
    // - 1 byte: version
    // - 1 byte: profile
    // - 1 byte: profile compatibility
    // - 1 byte: level
    // - 1 byte: NALU length size - 1 (masked with 0x03)
    // - 1 byte: number of SPS (masked with 0x1F)
    // - SPS entries: 2 byte length + data
    // - 1 byte: number of PPS
    // - PPS entries: 2 byte length + data

    let num_sps = (data[5] & 0x1F) as usize;
    let mut pos = 6;

    let mut sps = Vec::new();
    for _ in 0..num_sps {
        if pos + 2 > data.len() {
            return None;
        }
        let sps_len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2;
        if pos + sps_len > data.len() {
            return None;
        }
        // Add Annex B start code
        sps.extend_from_slice(&[0, 0, 0, 1]);
        sps.extend_from_slice(&data[pos..pos + sps_len]);
        pos += sps_len;
    }

    if pos >= data.len() {
        return None;
    }
    let num_pps = data[pos] as usize;
    pos += 1;

    let mut pps = Vec::new();
    for _ in 0..num_pps {
        if pos + 2 > data.len() {
            return None;
        }
        let pps_len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2;
        if pos + pps_len > data.len() {
            return None;
        }
        // Add Annex B start code
        pps.extend_from_slice(&[0, 0, 0, 1]);
        pps.extend_from_slice(&data[pos..pos + pps_len]);
        pos += pps_len;
    }

    Some((sps, pps))
}

/// Parse hvcC (H.265 codec configuration) to extract VPS/SPS/PPS as Annex B format.
fn parse_hvcc(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 23 {
        return None;
    }

    // Check if it starts with a box header containing "hvcC"
    let offset = if data.len() > 8 && &data[4..8] == b"hvcC" {
        8
    } else {
        0
    };

    let data = &data[offset..];
    if data.len() < 23 {
        return None;
    }

    // hvcC structure:
    // - 1 byte: version
    // - 12 bytes: profile_tier_level
    // - ... (configuration fields)
    // - 1 byte: numOfArrays at offset 22

    let num_arrays = data[22] as usize;
    let mut pos = 23;
    let mut csd = Vec::new();

    for _ in 0..num_arrays {
        if pos + 3 > data.len() {
            break;
        }

        // Skip array completeness and NAL type
        pos += 1;

        let num_nalus = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2;

        for _ in 0..num_nalus {
            if pos + 2 > data.len() {
                break;
            }
            let nalu_len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
            pos += 2;
            if pos + nalu_len > data.len() {
                break;
            }
            // Add Annex B start code
            csd.extend_from_slice(&[0, 0, 0, 1]);
            csd.extend_from_slice(&data[pos..pos + nalu_len]);
            pos += nalu_len;
        }
    }

    if csd.is_empty() {
        None
    } else {
        Some(csd)
    }
}
