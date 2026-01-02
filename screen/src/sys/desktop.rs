//! Desktop platform implementation (Windows/Linux/macOS).
//!
//! Uses the `screenshots` crate for screen capture and the `image` crate for PNG encoding.
//! HEIF/AVIF encoding is not supported on desktop platforms (use macOS native APIs).

use crate::screenshot::ImageFormat;
use crate::{Error, ScreenInfo, Screenshot};
use std::io::Cursor;

/// Enumerate all screens.
pub fn screens() -> Result<Vec<ScreenInfo>, Error> {
    let screens = screenshots::Screen::all().map_err(|e| Error::Platform(e.to_string()))?;

    let mut infos = Vec::with_capacity(screens.len());
    for screen in &screens {
        infos.push(ScreenInfo {
            id: screen.display_info.id,
            name: format!("Screen {}", screen.display_info.id),
            width: screen.display_info.width,
            height: screen.display_info.height,
            scale_factor: screen.display_info.scale_factor,
            is_primary: screen.display_info.is_primary,
        });
    }

    Ok(infos)
}

/// Get screen brightness (stub - brightness crate has build issues).
#[allow(clippy::unused_async)]
pub async fn get_brightness() -> Result<f32, Error> {
    // brightness crate is currently broken on macOS (build failure).
    Ok(1.0)
}

/// Set screen brightness (stub - brightness crate has build issues).
#[allow(clippy::unused_async)]
pub async fn set_brightness(_val: f32) -> Result<(), Error> {
    // brightness crate is currently broken.
    Ok(())
}

/// Capture a screenshot of the specified display.
pub fn screenshot(display: &ScreenInfo, format: ImageFormat) -> Result<Screenshot, Error> {
    // HEIF/AVIF not supported on desktop (Windows/Linux)
    // macOS uses the Apple module for these formats
    #[cfg(not(target_os = "macos"))]
    if matches!(format, ImageFormat::Heif | ImageFormat::Avif) {
        return Err(Error::Unsupported);
    }

    // On macOS, delegate HEIF/AVIF to Apple native APIs
    #[cfg(target_os = "macos")]
    if matches!(format, ImageFormat::Heif | ImageFormat::Avif) {
        return super::apple::screenshot(display, format);
    }

    // Find the screen by ID
    let screens = screenshots::Screen::all().map_err(|e| Error::Platform(e.to_string()))?;
    let screen = screens
        .iter()
        .find(|s| s.display_info.id == display.id)
        .ok_or(Error::MonitorNotFound)?;

    // Capture
    let image = screen
        .capture()
        .map_err(|e| Error::Platform(e.to_string()))?;

    let width = image.width();
    let height = image.height();

    // Encode as PNG
    let mut buffer = Vec::new();
    let mut cursor = Cursor::new(&mut buffer);
    image
        .write_to(&mut cursor, screenshots::image::ImageFormat::Png)
        .map_err(|e| Error::Encoding(e.to_string()))?;

    Ok(Screenshot {
        data: buffer,
        width,
        height,
        format: ImageFormat::Png,
    })
}

// ============================================================================
// ScreenStreamInner for Windows/Linux (GPU upload path)
// macOS uses the apple module for streaming
// ============================================================================

#[cfg(not(target_os = "macos"))]
use crate::frame::ScreenFrame;
#[cfg(not(target_os = "macos"))]
use crate::stream::StreamConfig;
#[cfg(not(target_os = "macos"))]
use std::sync::Arc;
#[cfg(not(target_os = "macos"))]
use wgpu::{Device, Extent3d, Queue, TextureDimension, TextureFormat, TextureUsages};

/// Screen stream for Windows/Linux.
///
/// Uses the `screenshots` crate for capture with GPU texture upload.
#[cfg(not(target_os = "macos"))]
pub struct ScreenStreamInner {
    screen: screenshots::Screen,
    device: Arc<Device>,
    queue: Arc<Queue>,
    width: u32,
    height: u32,
}

#[cfg(not(target_os = "macos"))]
impl ScreenStreamInner {
    /// Create a new screen stream.
    pub fn new(
        display: &ScreenInfo,
        device: Arc<Device>,
        queue: Arc<Queue>,
        _config: &StreamConfig,
    ) -> Result<Self, Error> {
        let screens = screenshots::Screen::all().map_err(|e| Error::Platform(e.to_string()))?;
        let screen = screens
            .into_iter()
            .find(|s| s.display_info.id == display.id)
            .ok_or(Error::MonitorNotFound)?;

        let width = display.width;
        let height = display.height;

        Ok(Self {
            screen,
            device,
            queue,
            width,
            height,
        })
    }

    /// Capture next frame asynchronously.
    pub async fn next_frame(&self) -> Option<ScreenFrame> {
        self.try_next_frame()
    }

    /// Try to capture a frame without blocking.
    pub fn try_next_frame(&self) -> Option<ScreenFrame> {
        let image = self.screen.capture().ok()?;
        let width = image.width();
        let height = image.height();
        let rgba = image.into_raw();

        // Create GPU texture
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ScreenCapture"),
            size: Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8UnormSrgb,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Upload data
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        let timestamp_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);

        Some(ScreenFrame::from_texture(
            Arc::new(texture),
            width,
            height,
            TextureFormat::Rgba8UnormSrgb,
            timestamp_ns,
        ))
    }

    /// Get the capture dimensions.
    pub const fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}
