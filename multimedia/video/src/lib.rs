//! Cross-platform video muxing, demuxing, and playback.
//!
//! This crate provides:
//! - **Muxing**: Write H.264/H.265 video to MP4/MOV containers
//! - **Demuxing**: Read video samples from containers
//! - **Hardware Decode**: Uses `waterkit-codec` for hardware-accelerated decode
//! - **wgpu Integration**: Render decoded frames to GPU textures
//!
//! # Video Playback Example
//!
//! ```ignore
//! use waterkit_video::{VideoPlayer};
//! use std::sync::Arc;
//!
//! let player = VideoPlayer::open("video.mp4", device.clone(), queue.clone())?;
//!
//! while let Some(frame) = player.next_frame()? {
//!     // Use frame.gpu_frame for rendering
//!     let y_texture = frame.gpu_frame.y_texture();
//!     let uv_texture = frame.gpu_frame.uv_texture();
//! }
//! ```

#![warn(missing_docs)]

mod demuxer;
mod muxer;
mod picture_in_picture;
mod player;

pub use demuxer::{
    EmbeddedSubtitleCodec, EmbeddedSubtitleCue, EmbeddedSubtitleTrack, VideoFrame, VideoReader,
    embedded_subtitle_tracks, read_embedded_subtitle_cues,
};
pub use muxer::{CodecType as MuxerCodecType, VideoFormat, VideoWriter};
pub use picture_in_picture::{
    PictureInPictureCommand, PictureInPictureControllerState, PictureInPictureHostId,
    enter_picture_in_picture, is_picture_in_picture_active, poll_picture_in_picture_command,
    sync_picture_in_picture_controller,
};
pub use player::{PlayerFrame, VideoPlayer};

/// Re-export wgpu for texture integration.
pub use wgpu;

/// Re-export codec crate for access to decoder.
pub use waterkit_codec as codec;

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
    Unsupported(String),
}
