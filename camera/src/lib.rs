//! Cross-platform camera streaming.
//!
//! This crate provides a unified API for camera enumeration and streaming
//! across iOS, macOS, Android, Windows, and Linux platforms with efficient
//! WGPU texture integration.

#![warn(missing_docs)]

mod sys;

use std::fmt;

/// Re-export wgpu for texture integration.
pub use wgpu;

/// Information about a camera device.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CameraInfo {
    /// Unique identifier for the camera.
    pub id: String,
    /// Human-readable name of the camera.
    pub name: String,
    /// Optional description or model information.
    pub description: Option<String>,
    /// Whether this is a front-facing camera (mobile).
    pub is_front_facing: bool,
}

/// Pixel format of camera frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FrameFormat {
    /// RGB format (3 bytes per pixel).
    #[default]
    Rgb,
    /// RGBA format (4 bytes per pixel).
    Rgba,
    /// BGRA format (4 bytes per pixel).
    Bgra,
    /// NV12 format (YUV 4:2:0, used on mobile).
    Nv12,
    /// YUYV/YUY2 format (YUV 4:2:2).
    Yuy2,
}

impl FrameFormat {
    /// Get bytes per pixel for this format.
    #[must_use]
    pub const fn bytes_per_pixel(&self) -> usize {
        match self {
            Self::Rgb => 3,
            Self::Rgba | Self::Bgra => 4,
            Self::Nv12 => 1, // Variable, but this is for the Y plane
            Self::Yuy2 => 2,
        }
    }

    /// Convert to WGPU texture format.
    #[must_use]
    pub const fn to_wgpu_format(&self) -> wgpu::TextureFormat {
        match self {
            Self::Rgba => wgpu::TextureFormat::Rgba8Unorm,
            Self::Bgra => wgpu::TextureFormat::Bgra8Unorm,
            Self::Rgb | Self::Nv12 | Self::Yuy2 => wgpu::TextureFormat::Rgba8Unorm, // Converted
        }
    }
}

/// Camera frame data.
#[derive(Clone)]
pub struct CameraFrame {
    /// Raw pixel data.
    pub data: Vec<u8>,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Pixel format of the frame.
    pub format: FrameFormat,
    /// Optional native handle for zero-copy access (platform specific).
    pub native_handle: Option<sys::NativeHandle>,
}

impl fmt::Debug for CameraFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CameraFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.format)
            .field("data_len", &self.data.len())
            .field("has_native_handle", &self.native_handle.is_some())
            .finish()
    }
}

impl CameraFrame {
    /// Create a new camera frame.
    #[must_use]
    pub fn new(
        data: Vec<u8>,
        width: u32,
        height: u32,
        format: FrameFormat,
        native_handle: Option<sys::NativeHandle>,
    ) -> Self {
        Self {
            data,
            width,
            height,
            format,
            native_handle,
        }
    }

    /// Convert frame to RGBA format if needed.
    #[must_use]
    pub fn to_rgba(&self) -> Vec<u8> {
        match self.format {
            FrameFormat::Rgba => self.data.clone(),
            FrameFormat::Bgra => {
                // BGRA -> RGBA: swap R and B channels
                let mut rgba = self.data.clone();
                for chunk in rgba.chunks_exact_mut(4) {
                    chunk.swap(0, 2);
                }
                rgba
            }
            FrameFormat::Rgb => {
                // RGB -> RGBA: add alpha channel
                let mut rgba = Vec::with_capacity(self.data.len() / 3 * 4);
                for chunk in self.data.chunks_exact(3) {
                    rgba.extend_from_slice(chunk);
                    rgba.push(255);
                }
                rgba
            }
            FrameFormat::Nv12 | FrameFormat::Yuy2 => {
                // YUV conversion would be done here
                // For now, return empty - platform code should convert before returning
                vec![0; (self.width * self.height * 4) as usize]
            }
        }
    }

    /// Write frame data to a WGPU texture.
    ///
    /// The texture must be created with the appropriate size and format.
    /// Use `format.to_wgpu_format()` when creating the texture.
    pub fn write_to_texture(&self, queue: &wgpu::Queue, texture: &wgpu::Texture) {
        let rgba_data = self.to_rgba();

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba_data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.width * 4),
                rows_per_image: Some(self.height),
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Create a WGPU texture suitable for this frame.
    #[must_use]
    pub fn create_texture(&self, device: &wgpu::Device) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("camera_frame_texture"),
            size: wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format.to_wgpu_format(),
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }
}

/// Errors that can occur with camera operations.
#[derive(Debug, Clone)]
pub enum CameraError {
    /// Camera is not supported on this platform.
    NotSupported,
    /// Failed to enumerate cameras.
    EnumerationFailed(String),
    /// Camera not found.
    NotFound(String),
    /// Failed to open camera.
    OpenFailed(String),
    /// Failed to start camera.
    StartFailed(String),
    /// Failed to capture frame.
    CaptureFailed(String),
    /// Permission denied.
    PermissionDenied,
    /// Camera is already in use.
    AlreadyInUse,
    /// An unknown error occurred.
    Unknown(String),
}

impl fmt::Display for CameraError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotSupported => write!(f, "camera not supported on this platform"),
            Self::EnumerationFailed(msg) => write!(f, "failed to enumerate cameras: {msg}"),
            Self::NotFound(id) => write!(f, "camera not found: {id}"),
            Self::OpenFailed(msg) => write!(f, "failed to open camera: {msg}"),
            Self::StartFailed(msg) => write!(f, "failed to start camera: {msg}"),
            Self::CaptureFailed(msg) => write!(f, "failed to capture frame: {msg}"),
            Self::PermissionDenied => write!(f, "camera permission denied"),
            Self::AlreadyInUse => write!(f, "camera is already in use"),
            Self::Unknown(msg) => write!(f, "unknown error: {msg}"),
        }
    }
}

impl std::error::Error for CameraError {}

/// Camera resolution configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Resolution {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl Resolution {
    /// Standard 720p resolution.
    pub const HD: Self = Self {
        width: 1280,
        height: 720,
    };

    /// Standard 1080p resolution.
    pub const FULL_HD: Self = Self {
        width: 1920,
        height: 1080,
    };

    /// Standard 4K resolution.
    pub const UHD: Self = Self {
        width: 3840,
        height: 2160,
    };
}

/// Camera controller.
#[derive(Debug)]
pub struct Camera {
    inner: sys::CameraInner,
}

impl Camera {
    /// List available cameras on the system.
    ///
    /// # Errors
    /// Returns an error if camera enumeration fails.
    pub fn list() -> Result<Vec<CameraInfo>, CameraError> {
        sys::CameraInner::list()
    }

    /// Open a camera by its ID.
    ///
    /// # Errors
    /// Returns an error if the camera cannot be opened.
    pub fn open(camera_id: &str) -> Result<Self, CameraError> {
        Ok(Self {
            inner: sys::CameraInner::open(camera_id)?,
        })
    }

    /// Open the default camera.
    ///
    /// On desktop, this is typically the first webcam.
    /// On mobile, this is typically the back camera.
    ///
    /// # Errors
    /// Returns an error if no camera is available or it cannot be opened.
    pub fn open_default() -> Result<Self, CameraError> {
        let cameras = Self::list()?;
        let camera = cameras.first().ok_or(CameraError::NotFound("no cameras available".into()))?;
        Self::open(&camera.id)
    }

    /// Start capturing frames.
    ///
    /// # Errors
    /// Returns an error if the camera cannot be started.
    pub fn start(&mut self) -> Result<(), CameraError> {
        self.inner.start()
    }

    /// Stop capturing frames.
    ///
    /// # Errors
    /// Returns an error if the camera cannot be stopped.
    pub fn stop(&mut self) -> Result<(), CameraError> {
        self.inner.stop()
    }

    /// Get the next captured frame.
    ///
    /// This may block until a frame is available.
    ///
    /// # Errors
    /// Returns an error if frame capture fails.
    pub fn get_frame(&mut self) -> Result<CameraFrame, CameraError> {
        self.inner.get_frame()
    }

    /// Set the desired resolution.
    ///
    /// The actual resolution may differ based on camera capabilities.
    ///
    /// # Errors
    /// Returns an error if the resolution cannot be set.
    pub fn set_resolution(&mut self, resolution: Resolution) -> Result<(), CameraError> {
        self.inner.set_resolution(resolution)
    }

    /// Get the current resolution.
    #[must_use]
    pub fn resolution(&self) -> Resolution {
        self.inner.resolution()
    }

    /// Get the number of dropped frames since start.
    pub fn dropped_frame_count(&self) -> u64 {
        self.inner.dropped_frame_count()
    }

    /// Enable or disable HDR mode.
    ///
    /// # Errors
    /// Returns `NotSupported` if the camera or backend does not support HDR/HLG.
    pub fn set_hdr(&self, enabled: bool) -> Result<(), CameraError> {
        self.inner.set_hdr(enabled)
    }

    /// Check if HDR mode is currently enabled.
    #[must_use]
    pub fn hdr_enabled(&self) -> bool {
        self.inner.hdr_enabled()
    }
}
