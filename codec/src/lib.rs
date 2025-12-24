//! High-performance video/image codec support.
//!
//! This crate provides hardware-accelerated encoding and decoding for video and images.
//! It abstracts over platform-specific APIs:
//! - **Apple**: `VideoToolbox`
//! - **Android**: `MediaCodec`
//! - **Windows**: Media Foundation
//! - **Linux**: (TODO: GStreamer/VA-API)
//!
//! It also provides software fallback for modern codecs like AV1 via `rav1e` and `dav1d`.

#![warn(missing_docs)]

/// Platform-specific implementations.
pub mod sys;

#[cfg(feature = "av1")]
pub mod av1;

use std::sync::Arc;
use thiserror::Error;

/// Common error type for codec operations.
#[derive(Debug, Error)]
pub enum CodecError {
    /// The codec or format is not supported.
    #[error("unsupported codec or format: {0}")]
    Unsupported(String),
    /// Initialization failed.
    #[error("initialization failed: {0}")]
    InitializationFailed(String),
    /// Encoding failed.
    #[error("encoding failed: {0}")]
    EncodingFailed(String),
    /// Decoding failed.
    #[error("decoding failed: {0}")]
    DecodingFailed(String),
    /// An unknown error occurred.
    #[error("unknown error: {0}")]
    Unknown(String),
}

/// Supported codec types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodecType {
    /// H.264 (AVC)
    H264,
    /// H.265 (HEVC)
    H265,
    /// VP8
    Vp8,
    /// VP9
    Vp9,
    /// AV1
    Av1,
}

/// Generic Video Encoder trait.
pub trait VideoEncoder: Send + Sync {
    /// Encode a frame.
    ///
    /// # Errors
    ///
    /// Returns `CodecError::EncodingFailed` if encoding fails.
    fn encode(&mut self, frame: &Frame) -> Result<Vec<u8>, CodecError>;
}

/// Generic Video Decoder trait.
pub trait VideoDecoder: Send + Sync {
    /// Decode a packet into one or more frames.
    ///
    /// # Errors
    ///
    /// Returns `CodecError::DecodingFailed` if decoding fails.
    fn decode(&mut self, data: &[u8]) -> Result<Vec<Frame>, CodecError>;
}

/// A single frame of video or image data.
/// Similar to `camera::CameraFrame` but decoupled.
#[derive(Clone)]
pub struct Frame {
    /// Raw data (e.g. RGBA, NV12).
    pub data: Arc<Vec<u8>>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Format of the data.
    pub format: PixelFormat,
    /// Timestamp in nanoseconds.
    pub timestamp_ns: u64,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.format)
            .field("timestamp_ns", &self.timestamp_ns)
            .finish_non_exhaustive()
    }
}

/// Pixel format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    /// RGBA 8-bit.
    Rgba,
    /// BGRA 8-bit.
    Bgra,
    /// NV12 (YUV 4:2:0 bi-planar).
    Nv12,
    /// I420 (YUV 4:2:0 planar).
    I420,
}
