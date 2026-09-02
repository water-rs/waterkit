//! Desktop platform implementation (Windows/Linux/macOS).
//!
//! Uses `xcap` for screen capture and PNG encoding.
//! HEIF/AVIF encoding is not supported on desktop platforms (use macOS native APIs).

use crate::screenshot::ImageFormat;
use crate::{Error, ScreenInfo, Screenshot};
#[cfg(any(target_os = "linux", target_os = "windows"))]
use brightness::blocking::{Brightness, brightness_devices};
use std::io::Cursor;
#[cfg(target_os = "linux")]
use wayland_sys as _;

// Taking a reference here, as `needless_pass_by_value` suggests, would stop
// this being usable as `.map_err(map_xcap_error)` — which is its only caller
// shape, a dozen times over in this file.
#[expect(
    clippy::needless_pass_by_value,
    reason = "must own its argument to be passed directly to `Result::map_err`."
)]
fn map_xcap_error(error: xcap::XCapError) -> Error {
    Error::Platform(error.to_string())
}

fn monitors() -> Result<Vec<xcap::Monitor>, Error> {
    xcap::Monitor::all().map_err(map_xcap_error)
}

fn monitor_by_id(id: u32) -> Result<xcap::Monitor, Error> {
    for monitor in monitors()? {
        if monitor.id().map_err(map_xcap_error)? == id {
            return Ok(monitor);
        }
    }
    Err(Error::MonitorNotFound)
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn map_brightness_error(error: &brightness::Error) -> Error {
    Error::Platform(format!("brightness backend error: {error}"))
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn first_brightness_device() -> Result<brightness::blocking::BrightnessDevice, Error> {
    let mut first_error = None;
    for device in brightness_devices() {
        match device {
            Ok(device) => return Ok(device),
            Err(error) if first_error.is_none() => first_error = Some(error),
            Err(_) => {}
        }
    }

    first_error.map_or(Err(Error::MonitorNotFound), |error| {
        Err(map_brightness_error(&error))
    })
}

/// Enumerate all screens.
pub fn screens() -> Result<Vec<ScreenInfo>, Error> {
    let monitors = monitors()?;
    let mut infos = Vec::with_capacity(monitors.len());
    for monitor in monitors {
        infos.push(ScreenInfo::new(
            monitor.id().map_err(map_xcap_error)?,
            monitor.name().map_err(map_xcap_error)?,
            monitor.width().map_err(map_xcap_error)?,
            monitor.height().map_err(map_xcap_error)?,
            monitor.scale_factor().map_err(map_xcap_error)?,
            monitor.is_primary().map_err(map_xcap_error)?,
        ));
    }

    Ok(infos)
}

/// Returns the maximum refresh rate across connected desktop displays.
pub fn max_refresh_rate_hz() -> Result<f32, Error> {
    let mut max_refresh_rate = None;
    for monitor in monitors()? {
        let refresh_rate = monitor.frequency().map_err(map_xcap_error)?;
        if refresh_rate.is_finite() && refresh_rate > 0.0 {
            max_refresh_rate = Some(
                max_refresh_rate.map_or(refresh_rate, |current: f32| current.max(refresh_rate)),
            );
        }
    }
    max_refresh_rate.ok_or(Error::MonitorNotFound)
}

#[allow(clippy::unused_async)]
#[allow(clippy::cast_precision_loss)]
pub async fn get_brightness() -> Result<f32, Error> {
    #[cfg(target_os = "macos")]
    {
        super::apple::get_macos_brightness()
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        blocking::unblock(|| {
            let device = first_brightness_device()?;
            let percentage = device.get().map_err(|error| map_brightness_error(&error))?;
            Ok(((percentage as f32) / 100.0).clamp(0.0, 1.0))
        })
        .await
    }
}

#[allow(clippy::unused_async)]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub async fn set_brightness(val: f32) -> Result<(), Error> {
    #[cfg(target_os = "macos")]
    {
        super::apple::set_macos_brightness(val)
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        blocking::unblock(move || {
            let device = first_brightness_device()?;
            let percentage = (val.clamp(0.0, 1.0) * 100.0).round() as u32;
            device
                .set(percentage.min(100))
                .map_err(|error| map_brightness_error(&error))
        })
        .await
    }
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

    let monitor = monitor_by_id(display.id())?;
    let image = monitor.capture_image().map_err(map_xcap_error)?;

    let width = image.width();
    let height = image.height();

    // Encode as PNG
    let mut buffer = Vec::new();
    let mut cursor = Cursor::new(&mut buffer);
    image
        .write_to(&mut cursor, xcap::image::ImageFormat::Png)
        .map_err(|e| Error::Encoding(e.to_string()))?;

    Ok(Screenshot::new(buffer, width, height, ImageFormat::Png))
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
/// Uses `xcap` for capture with GPU texture upload.
#[cfg(not(target_os = "macos"))]
pub struct ScreenStreamInner {
    monitor: xcap::Monitor,
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
        let monitor = monitor_by_id(display.id())?;

        let width = display.width();
        let height = display.height();

        Ok(Self {
            monitor,
            device,
            queue,
            width,
            height,
        })
    }

    /// Capture next frame asynchronously.
    #[allow(clippy::unused_async)]
    #[allow(
        clippy::future_not_send,
        reason = "the Windows capture session is a thread-affine `*mut c_void`, so `ScreenStream` is deliberately not `Sync` and these futures cannot be `Send`."
    )]
    pub async fn next_frame(&self) -> Option<ScreenFrame> {
        self.try_next_frame()
    }

    /// Try to capture a frame without blocking.
    pub fn try_next_frame(&self) -> Option<ScreenFrame> {
        let image = self.monitor.capture_image().ok()?;
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
            .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));

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
