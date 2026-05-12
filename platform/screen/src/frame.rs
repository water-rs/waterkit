//! GPU-backed screen capture frame.

use std::sync::Arc;
use wgpu::{Texture, TextureFormat, TextureView, TextureViewDescriptor};

/// GPU-backed screen capture frame.
///
/// The frame data lives in GPU memory and can be used directly for rendering
/// without any CPU copies.
#[derive(Debug)]
pub struct ScreenFrame {
    texture: Arc<Texture>,
    width: u32,
    height: u32,
    format: TextureFormat,
    timestamp_ns: u64,
}

impl ScreenFrame {
    /// Create a new frame from an existing wgpu texture.
    #[must_use]
    pub const fn from_texture(
        texture: Arc<Texture>,
        width: u32,
        height: u32,
        format: TextureFormat,
        timestamp_ns: u64,
    ) -> Self {
        Self {
            texture,
            width,
            height,
            format,
            timestamp_ns,
        }
    }

    /// Get a reference to the underlying wgpu texture.
    #[must_use]
    pub fn texture(&self) -> &Texture {
        &self.texture
    }

    /// Get the frame width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Get the frame height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Get the texture format.
    #[must_use]
    pub const fn format(&self) -> TextureFormat {
        self.format
    }

    /// Get the capture timestamp in nanoseconds.
    #[must_use]
    pub const fn timestamp_ns(&self) -> u64 {
        self.timestamp_ns
    }

    /// Create a texture view for rendering.
    #[must_use]
    pub fn create_view(&self) -> TextureView {
        self.texture.create_view(&TextureViewDescriptor::default())
    }
}
