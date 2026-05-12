//! Apple platform implementation (macOS/iOS).
//!
//! Uses `ScreenCaptureKit` for screen capture on macOS.
//! Uses `UIKit` for iOS screenshot capture.

use crate::frame::ScreenFrame;
use crate::screenshot::ImageFormat;
use crate::stream::StreamConfig;
use crate::{Error, ScreenInfo, Screenshot};
use std::sync::Arc;
use wgpu::{Device, Queue};
#[cfg(target_os = "macos")]
use wgpu::{Extent3d, TextureDimension, TextureFormat, TextureUsages};

#[swift_bridge::bridge]
mod ffi {
    extern "Swift" {
        // Screenshot capture (format: 0=PNG, 1=AVIF, 2=HEIF)
        fn capture_screenshot(format: u8) -> Vec<u8>;

        // Brightness
        fn get_screen_brightness() -> f32;
        fn set_screen_brightness(value: f32) -> bool;

        // Screen stream
        fn init_screen_stream(display_id: u32, target_fps: u32, show_cursor: bool) -> bool;
        fn stop_screen_stream();
        fn get_iosurface_ptr() -> u64;
        fn get_iosurface_sequence() -> u32;
        fn get_frame_width() -> u32;
        fn get_frame_height() -> u32;
        fn get_frame_timestamp_ns() -> u64;
    }
}

// ============================================================================
// Screenshot
// ============================================================================

/// Capture a screenshot with the specified format.
pub fn screenshot(display: &ScreenInfo, format: ImageFormat) -> Result<Screenshot, Error> {
    let format_code = match format {
        ImageFormat::Png => 0,
        ImageFormat::Avif => 1,
        ImageFormat::Heif => 2,
    };

    let data = ffi::capture_screenshot(format_code);

    if data.is_empty() {
        return Err(Error::Platform("Screenshot capture failed".into()));
    }

    Ok(Screenshot::new(
        data,
        display.width(),
        display.height(),
        format,
    ))
}

// ============================================================================
// Screens (iOS only - macOS uses desktop module)
// ============================================================================

#[cfg(target_os = "ios")]
#[allow(clippy::unnecessary_wraps)]
pub fn screens() -> Result<Vec<ScreenInfo>, Error> {
    // iOS has a single main screen conceptually
    Ok(vec![ScreenInfo::new(
        0,
        "Main Screen".into(),
        0, // Would need UIScreen.main.bounds
        0,
        1.0,
        true,
    )])
}

// ============================================================================
// Brightness (iOS only - macOS uses desktop module)
// ============================================================================

#[cfg(target_os = "ios")]
#[allow(clippy::unused_async)]
pub async fn get_brightness() -> Result<f32, Error> {
    let value = ffi::get_screen_brightness();
    if !(0.0..=1.0).contains(&value) {
        return Err(Error::Platform(format!(
            "invalid iOS brightness value from platform bridge: {value}"
        )));
    }

    Ok(value)
}

#[cfg(target_os = "ios")]
#[allow(clippy::unused_async)]
pub async fn set_brightness(val: f32) -> Result<(), Error> {
    if ffi::set_screen_brightness(val.clamp(0.0, 1.0)) {
        Ok(())
    } else {
        Err(Error::Platform(
            "failed to set iOS screen brightness".into(),
        ))
    }
}

#[cfg(target_os = "macos")]
pub fn get_macos_brightness() -> Result<f32, Error> {
    let value = ffi::get_screen_brightness();
    if !(0.0..=1.0).contains(&value) {
        return Err(Error::Platform(format!(
            "invalid macOS brightness value from platform bridge: {value}"
        )));
    }

    Ok(value)
}

#[cfg(target_os = "macos")]
pub fn set_macos_brightness(value: f32) -> Result<(), Error> {
    if ffi::set_screen_brightness(value.clamp(0.0, 1.0)) {
        Ok(())
    } else {
        Err(Error::Platform(
            "failed to set macOS screen brightness".into(),
        ))
    }
}

// ============================================================================
// ScreenStreamInner
// ============================================================================

/// Screen stream with IOSurface-based capture on macOS.
pub struct ScreenStreamInner {
    width: u32,
    height: u32,
    #[cfg(target_os = "macos")]
    last_sequence: std::sync::atomic::AtomicU32,
    #[cfg(target_os = "macos")]
    device: Arc<Device>,
    #[cfg(target_os = "macos")]
    queue: Arc<Queue>,
}

impl ScreenStreamInner {
    /// Create a new screen stream.
    #[cfg(target_os = "macos")]
    pub fn new(
        display: &ScreenInfo,
        device: Arc<Device>,
        queue: Arc<Queue>,
        config: &StreamConfig,
    ) -> Result<Self, Error> {
        let success = ffi::init_screen_stream(display.id(), config.target_fps, config.show_cursor);

        if !success {
            return Err(Error::Platform("Failed to initialize screen stream".into()));
        }

        Ok(Self {
            width: display.width(),
            height: display.height(),
            last_sequence: std::sync::atomic::AtomicU32::new(0),
            device,
            queue,
        })
    }

    /// Create a new screen stream (iOS - unsupported).
    #[cfg(target_os = "ios")]
    #[allow(clippy::unnecessary_wraps)]
    pub fn new(
        display: &ScreenInfo,
        _device: Arc<Device>,
        _queue: Arc<Queue>,
        _config: &StreamConfig,
    ) -> Result<Self, Error> {
        Ok(Self {
            width: display.width(),
            height: display.height(),
        })
    }

    /// Get next frame asynchronously.
    #[allow(clippy::unused_async)]
    pub async fn next_frame(&self) -> Option<ScreenFrame> {
        self.try_next_frame()
    }

    /// Try to get a frame without blocking.
    #[cfg(target_os = "macos")]
    pub fn try_next_frame(&self) -> Option<ScreenFrame> {
        use std::sync::atomic::Ordering;

        let sequence = ffi::get_iosurface_sequence();
        let last = self.last_sequence.load(Ordering::Relaxed);

        // No new frame
        if sequence == last || sequence == 0 {
            return None;
        }

        let iosurface_ptr = ffi::get_iosurface_ptr();
        if iosurface_ptr == 0 {
            return None;
        }

        let width = ffi::get_frame_width();
        let height = ffi::get_frame_height();
        let timestamp_ns = ffi::get_frame_timestamp_ns();

        // Read BGRA data from IOSurface and upload to GPU
        let bgra_data = read_iosurface_bgra(iosurface_ptr, width, height)?;

        self.last_sequence.store(sequence, Ordering::Relaxed);

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
            format: TextureFormat::Bgra8UnormSrgb,
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
            &bgra_data,
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

        Some(ScreenFrame::from_texture(
            Arc::new(texture),
            width,
            height,
            TextureFormat::Bgra8UnormSrgb,
            timestamp_ns,
        ))
    }

    #[cfg(target_os = "ios")]
    #[allow(clippy::unused_self)]
    pub const fn try_next_frame(&self) -> Option<ScreenFrame> {
        // iOS doesn't support screen capture streaming
        None
    }

    /// Get capture dimensions.
    pub const fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

impl Drop for ScreenStreamInner {
    fn drop(&mut self) {
        ffi::stop_screen_stream();
    }
}

// ============================================================================
// IOSurface BGRA reading (macOS only)
// ============================================================================

#[cfg(target_os = "macos")]
fn read_iosurface_bgra(iosurface_ptr: u64, width: u32, height: u32) -> Option<Vec<u8>> {
    use objc2_io_surface::{IOSurfaceLockOptions, IOSurfaceRef};
    use std::ptr;

    let expected_size = (width * height * 4) as usize;
    let mut bgra_data = vec![0u8; expected_size];

    unsafe {
        // Get IOSurfaceRef from pointer
        let surface_ref = (iosurface_ptr as *const IOSurfaceRef).as_ref()?;

        // Lock for reading
        surface_ref.lock(IOSurfaceLockOptions::ReadOnly, ptr::null_mut());

        // Get base address and stride
        let base_address = surface_ref.base_address();
        let bytes_per_row = surface_ref.bytes_per_row();
        let src = base_address.as_ptr().cast::<u8>();
        let row_bytes = (width * 4) as usize;

        // Copy row by row (handle stride)
        if bytes_per_row == row_bytes {
            // Fast path: no stride padding
            ptr::copy_nonoverlapping(src, bgra_data.as_mut_ptr(), expected_size);
        } else {
            // Handle stride
            for row in 0..height as usize {
                ptr::copy_nonoverlapping(
                    src.add(row * bytes_per_row),
                    bgra_data.as_mut_ptr().add(row * row_bytes),
                    row_bytes,
                );
            }
        }

        surface_ref.unlock(IOSurfaceLockOptions::ReadOnly, ptr::null_mut());
    }

    Some(bgra_data)
}
