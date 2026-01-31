//! Linux FFmpeg hardware encoding and decoding.
//!
//! Uses FFmpeg's hardware acceleration abstraction (`VA-API`, `VDPAU`, etc.)
//! for H.264 and H.265 video codec operations.

// FFmpeg types contain raw pointers but are safe to send between threads
#![allow(clippy::non_send_fields_in_send_ty)]
// These lints are overly strict for codec implementation
#![allow(
    dead_code,
    clippy::missing_const_for_fn,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::uninlined_format_args
)]

use crate::CodecError;
use ffmpeg_next as ffmpeg;
use ffmpeg_next::codec::context::Context;
use ffmpeg_next::format::Pixel;
use ffmpeg_next::software::scaling::{Context as ScalerContext, Flags};
use ffmpeg_next::util::frame::video::Video;
use ffmpeg_next::{Packet, codec, decoder, encoder};
use std::fmt;
use std::sync::Once;

static FFMPEG_INIT: Once = Once::new();

fn init_ffmpeg() {
    FFMPEG_INIT.call_once(|| {
        ffmpeg::init().expect("Failed to initialize FFmpeg");
    });
}

/// Internal codec type for Linux implementations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecType {
    H264,
    H265,
}

impl CodecType {
    fn decoder_name(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::H265 => "hevc",
        }
    }

    fn encoder_name(self) -> &'static str {
        match self {
            Self::H264 => "libx264",
            Self::H265 => "libx265",
        }
    }

    fn hw_decoder_name(self) -> &'static str {
        // Try hardware decoders first
        match self {
            Self::H264 => "h264_vaapi",
            Self::H265 => "hevc_vaapi",
        }
    }

    fn hw_encoder_name(self) -> &'static str {
        match self {
            Self::H264 => "h264_vaapi",
            Self::H265 => "hevc_vaapi",
        }
    }
}

/// Decoded frame from Linux FFmpeg (`NV12` format).
#[derive(Clone)]
pub struct LinuxFrame {
    /// `NV12` data: Y plane followed by interleaved UV plane.
    pub data: Vec<u8>,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Presentation timestamp in nanoseconds.
    pub timestamp_ns: u64,
}

impl fmt::Debug for LinuxFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LinuxFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("timestamp_ns", &self.timestamp_ns)
            .finish_non_exhaustive()
    }
}

/// Linux FFmpeg hardware decoder.
pub struct LinuxDecoder {
    decoder: decoder::Video,
    scaler: Option<ScalerContext>,
    codec_type: CodecType,
    width: u32,
    height: u32,
}

impl fmt::Debug for LinuxDecoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LinuxDecoder")
            .field("codec_type", &self.codec_type)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

unsafe impl Send for LinuxDecoder {}
unsafe impl Sync for LinuxDecoder {}

impl LinuxDecoder {
    /// Create a new Linux hardware decoder.
    pub fn new(
        codec_type: CodecType,
        config: Option<&[u8]>,
        width: u32,
        height: u32,
    ) -> Result<Self, CodecError> {
        init_ffmpeg();

        // Try hardware decoder first, fall back to software
        let codec = ffmpeg::decoder::find_by_name(codec_type.hw_decoder_name())
            .or_else(|| ffmpeg::decoder::find_by_name(codec_type.decoder_name()))
            .or_else(|| {
                ffmpeg::decoder::find(match codec_type {
                    CodecType::H264 => codec::Id::H264,
                    CodecType::H265 => codec::Id::HEVC,
                })
            })
            .ok_or_else(|| {
                CodecError::InitializationFailed(format!("No decoder found for {:?}", codec_type))
            })?;

        let context = Context::new_with_codec(codec);

        // Note: codec config (extradata) will be passed with first packet
        let _ = config;

        let decoder = context.decoder().video().map_err(|e| {
            CodecError::InitializationFailed(format!("Failed to create video decoder: {e}"))
        })?;

        Ok(Self {
            decoder,
            scaler: None,
            codec_type,
            width,
            height,
        })
    }

    /// Decode compressed video data.
    pub fn decode(&mut self, data: &[u8]) -> Result<Vec<LinuxFrame>, CodecError> {
        let mut packet = Packet::copy(data);
        packet.set_pts(Some(0));
        packet.set_dts(Some(0));

        self.decoder
            .send_packet(&packet)
            .map_err(|e| CodecError::DecodingFailed(format!("send_packet failed: {e}")))?;

        let mut frames = Vec::new();
        let mut decoded_frame = Video::empty();

        while self.decoder.receive_frame(&mut decoded_frame).is_ok() {
            let frame_width = decoded_frame.width();
            let frame_height = decoded_frame.height();
            let format = decoded_frame.format();

            // Update dimensions if the decoder reports different dimensions
            let width = if frame_width > 0 {
                frame_width
            } else {
                self.width
            };
            let height = if frame_height > 0 {
                frame_height
            } else {
                self.height
            };

            // Convert to NV12 if needed
            let nv12_data = if format == Pixel::NV12 {
                // Already NV12, extract directly
                extract_nv12(&decoded_frame, width, height)
            } else {
                // Need to convert - create or update scaler
                if self.scaler.is_none()
                    || self.scaler.as_ref().map(|s| s.input().width) != Some(width)
                {
                    self.scaler = Some(
                        ScalerContext::get(
                            format,
                            width,
                            height,
                            Pixel::NV12,
                            width,
                            height,
                            Flags::BILINEAR,
                        )
                        .map_err(|e| {
                            CodecError::DecodingFailed(format!("Scaler creation failed: {e}"))
                        })?,
                    );
                }

                let scaler = self.scaler.as_mut().unwrap();
                let mut nv12_frame = Video::empty();
                scaler
                    .run(&decoded_frame, &mut nv12_frame)
                    .map_err(|e| CodecError::DecodingFailed(format!("Scaling failed: {e}")))?;

                extract_nv12(&nv12_frame, width, height)
            };

            let timestamp_ns = decoded_frame
                .pts()
                .map_or(0, |pts| (pts as u64) * 1_000_000_000 / 90_000); // Assuming 90kHz timebase

            frames.push(LinuxFrame {
                data: nv12_data,
                width,
                height,
                timestamp_ns,
            });
        }

        Ok(frames)
    }
}

/// Linux FFmpeg hardware encoder.
pub struct LinuxEncoder {
    encoder: encoder::video::Video,
    scaler: Option<ScalerContext>,
    codec_type: CodecType,
    width: u32,
    height: u32,
    frame_count: i64,
    codec_config: Option<Vec<u8>>,
}

impl fmt::Debug for LinuxEncoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LinuxEncoder")
            .field("codec_type", &self.codec_type)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("frame_count", &self.frame_count)
            .finish_non_exhaustive()
    }
}

unsafe impl Send for LinuxEncoder {}
unsafe impl Sync for LinuxEncoder {}

impl LinuxEncoder {
    /// Create a new Linux hardware encoder.
    pub fn new(codec_type: CodecType, width: u32, height: u32) -> Result<Self, CodecError> {
        init_ffmpeg();

        // Try hardware encoder first, fall back to software
        let codec = ffmpeg::encoder::find_by_name(codec_type.hw_encoder_name())
            .or_else(|| ffmpeg::encoder::find_by_name(codec_type.encoder_name()))
            .or_else(|| {
                ffmpeg::encoder::find(match codec_type {
                    CodecType::H264 => codec::Id::H264,
                    CodecType::H265 => codec::Id::HEVC,
                })
            })
            .ok_or_else(|| {
                CodecError::InitializationFailed(format!("No encoder found for {:?}", codec_type))
            })?;

        let context = Context::new_with_codec(codec);

        // Create and configure encoder
        let mut encoder = context.encoder().video().map_err(|e| {
            CodecError::InitializationFailed(format!("Failed to get video encoder: {e}"))
        })?;

        encoder.set_width(width);
        encoder.set_height(height);
        encoder.set_format(Pixel::NV12);
        encoder.set_time_base((1, 30)); // 30 fps
        encoder.set_bit_rate(4_000_000); // 4 Mbps
        encoder.set_gop(30); // Keyframe every 1 second at 30fps

        Ok(Self {
            encoder,
            scaler: None,
            codec_type,
            width,
            height,
            frame_count: 0,
            codec_config: None,
        })
    }

    /// Encode `NV12` data to compressed video.
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

        // Create video frame from NV12 data
        let mut frame = Video::new(Pixel::NV12, self.width, self.height);

        // Copy Y plane
        let y_stride = frame.stride(0);
        let y_plane = frame.data_mut(0);
        for row in 0..self.height as usize {
            let src_start = row * self.width as usize;
            let dst_start = row * y_stride;
            y_plane[dst_start..dst_start + self.width as usize]
                .copy_from_slice(&nv12[src_start..src_start + self.width as usize]);
        }

        // Copy UV plane
        let uv_stride = frame.stride(1);
        let uv_plane = frame.data_mut(1);
        let uv_height = self.height as usize / 2;
        for row in 0..uv_height {
            let src_start = y_size + row * self.width as usize;
            let dst_start = row * uv_stride;
            uv_plane[dst_start..dst_start + self.width as usize]
                .copy_from_slice(&nv12[src_start..src_start + self.width as usize]);
        }

        frame.set_pts(Some(self.frame_count));
        self.frame_count += 1;

        // Send frame to encoder
        self.encoder
            .send_frame(&frame)
            .map_err(|e| CodecError::EncodingFailed(format!("send_frame failed: {e}")))?;

        // Collect encoded packets
        let mut encoded_data = Vec::new();
        let mut packet = Packet::empty();

        while self.encoder.receive_packet(&mut packet).is_ok() {
            encoded_data.extend_from_slice(packet.data().unwrap_or(&[]));
        }

        // Extract codec config from first packet if not yet captured
        if self.codec_config.is_none() && !encoded_data.is_empty() {
            // For H.264/H.265, the first few packets often contain SPS/PPS
            // This is a simplified extraction - proper implementation would parse NAL units
            self.codec_config = Some(encoded_data.clone());
        }

        Ok(encoded_data)
    }

    /// Get the codec configuration data if available.
    #[must_use]
    pub fn get_codec_config(&self) -> Option<Vec<u8>> {
        self.codec_config.clone()
    }
}

/// Extract `NV12` data from an FFmpeg video frame.
fn extract_nv12(frame: &Video, width: u32, height: u32) -> Vec<u8> {
    let y_size = (width * height) as usize;
    let uv_size = y_size / 2;
    let mut nv12_data = Vec::with_capacity(y_size + uv_size);

    // Copy Y plane (removing stride padding)
    let y_stride = frame.stride(0);
    let y_data = frame.data(0);
    for row in 0..height as usize {
        let start = row * y_stride;
        let end = start + width as usize;
        if end <= y_data.len() {
            nv12_data.extend_from_slice(&y_data[start..end]);
        } else {
            nv12_data.extend_from_slice(&y_data[start..]);
            nv12_data.resize(nv12_data.len() + (end - y_data.len()), 0);
        }
    }

    // Copy UV plane (already interleaved in NV12)
    let uv_stride = frame.stride(1);
    let uv_data = frame.data(1);
    let uv_height = height as usize / 2;
    for row in 0..uv_height {
        let start = row * uv_stride;
        let end = start + width as usize;
        if end <= uv_data.len() {
            nv12_data.extend_from_slice(&uv_data[start..end]);
        } else if start < uv_data.len() {
            nv12_data.extend_from_slice(&uv_data[start..]);
            nv12_data.resize(nv12_data.len() + (end - uv_data.len()), 0);
        }
    }

    nv12_data
}
