//! Cross-platform video muxing, demuxing, and playback.
//!
//! This crate provides:
//! - **Muxing**: Write H.264/H.265 video to MP4/MOV containers
//! - **Demuxing**: Read video samples from containers
//! - **Hardware Decode**: `VideoToolbox` (Apple), `MediaCodec` (Android)
//! - **wgpu Integration**: Render decoded frames to GPU textures

#![warn(missing_docs)]

mod muxer;
mod demuxer;

// Platform-specific (hardware decode) - to be implemented
// #[cfg(any(target_os = "macos", target_os = "ios"))]
// mod sys;

pub use muxer::{VideoWriter, VideoFormat, CodecType};
pub use demuxer::{VideoReader, VideoFrame};

/// Re-export wgpu for texture integration.
pub use wgpu;

/// Errors that can occur with video operations.
/// Errors that can occur with video operations.
#[derive(Debug, thiserror::Error)]
pub enum VideoError {
    /// IO error during file operations.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    
    /// MP4 container error.
    #[error(transparent)]
    Mp4(#[from] mp4::Error),
    
    /// Container format error.
    #[error("Container error: {0}")]
    Container(String),
    
    /// Codec error during encode/decode.
    #[error("Codec error: {0}")]
    Codec(String),
    
    /// Format not supported.
    #[error("Format not supported: {0}")]
    NotSupported(String),
}
