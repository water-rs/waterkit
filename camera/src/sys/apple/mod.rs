//! Apple platform (iOS/macOS) camera implementation using `AVCaptureSession`.
//!
//! Uses Metal texture interop for zero-copy frame rendering with wgpu.

// These functions need &mut self for API consistency across platforms
#![allow(clippy::needless_pass_by_ref_mut)]
// Async is required for API consistency across platforms
#![allow(clippy::unused_async)]
// Result is required for API consistency (other platforms may fail)
#![allow(clippy::unnecessary_wraps)]
// Self is needed for API consistency across platforms
#![allow(clippy::unused_self)]
// Const fn not needed for these accessors that may change
#![allow(clippy::missing_const_for_fn)]

use crate::{
    CameraCapabilities, CameraConfig, CameraControls, CameraError, CameraInfo, ExposureControl,
    ExposureMode, FlashMode, FocusControl, FocusMode, Frame, Photo, PixelFormat, Resolution,
    StabilizationMode, WhiteBalanceControl, WhiteBalanceMode,
};
use futures::StreamExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

#[swift_bridge::bridge]
mod ffi {
    enum CameraResultFFI {
        Success,
        NotSupported,
        EnumerationFailed,
        NotFound,
        OpenFailed,
        StartFailed,
        CaptureFailed,
        PermissionDenied,
        AlreadyInUse,
    }

    extern "Swift" {
        // Device enumeration
        fn camera_device_count() -> i32;
        fn camera_device_id(index: i32) -> String;
        fn camera_device_name(index: i32) -> String;
        fn camera_device_description(index: i32) -> String;
        fn camera_device_is_front(index: i32) -> bool;

        // Camera lifecycle
        fn camera_open(device_id: String) -> CameraResultFFI;
        fn camera_start() -> CameraResultFFI;
        fn camera_stop() -> CameraResultFFI;
        fn camera_is_streaming() -> bool;

        // Resolution
        fn camera_set_resolution(width: u32, height: u32) -> CameraResultFFI;
        fn camera_get_resolution_width() -> u32;
        fn camera_get_resolution_height() -> u32;

        // Capabilities
        fn camera_get_iso_min() -> f32;
        fn camera_get_iso_max() -> f32;
        fn camera_get_exposure_duration_min_ns() -> u64;
        fn camera_get_exposure_duration_max_ns() -> u64;
        fn camera_supports_exposure_compensation() -> bool;
        fn camera_supports_manual_focus() -> bool;
        fn camera_supports_manual_white_balance() -> bool;
        fn camera_get_zoom_min() -> f32;
        fn camera_get_zoom_max() -> f32;
        fn camera_supports_hdr() -> bool;
        fn camera_supports_standard_stabilization() -> bool;
        fn camera_supports_cinematic_stabilization() -> bool;
        fn camera_has_flash() -> bool;
        fn camera_has_torch() -> bool;

        // Exposure control
        fn camera_set_exposure_mode(mode: u8) -> CameraResultFFI;
        fn camera_set_iso(iso: f32) -> CameraResultFFI;
        fn camera_set_exposure_duration_ns(duration_ns: u64) -> CameraResultFFI;
        fn camera_set_exposure_compensation(ev: f32) -> CameraResultFFI;

        // Focus control
        fn camera_set_focus_mode(mode: u8) -> CameraResultFFI;
        fn camera_set_focus_distance(distance: f32) -> CameraResultFFI;
        fn camera_set_focus_point(x: f32, y: f32) -> CameraResultFFI;

        // White balance control
        fn camera_set_white_balance_mode(mode: u8) -> CameraResultFFI;
        fn camera_set_white_balance_temperature(kelvin: u32) -> CameraResultFFI;

        // Zoom control
        fn camera_set_zoom(factor: f32) -> CameraResultFFI;
        fn camera_get_zoom() -> f32;

        // Flash/Torch
        fn camera_set_flash_mode(mode: u8) -> CameraResultFFI;
        fn camera_set_torch_mode(enabled: bool) -> CameraResultFFI;

        // HDR
        fn camera_set_hdr(enabled: bool) -> CameraResultFFI;
        fn camera_get_hdr() -> bool;

        // Stabilization
        fn camera_set_stabilization_mode(mode: u8) -> CameraResultFFI;

        // Photo capture
        fn camera_take_photo() -> CameraResultFFI;
        fn camera_get_photo_len() -> i32;

        // Recording
        fn camera_start_recording(path: String) -> CameraResultFFI;
        fn camera_stop_recording() -> CameraResultFFI;
        fn camera_get_recording_duration_ms() -> u64;
    }
}

// External C functions
unsafe extern "C" {
    fn camera_set_frame_callback(callback: extern "C" fn(u64, u32, u32, u64));
    fn camera_clear_frame_callback();
    fn camera_release_pixelbuffer(handle: u64);
    fn camera_copy_photo_data(buffer: *mut u8, size: u64);
}

fn convert_result(result: ffi::CameraResultFFI, context: &str) -> Result<(), CameraError> {
    match result {
        ffi::CameraResultFFI::Success => Ok(()),
        ffi::CameraResultFFI::NotSupported => {
            Err(CameraError::ControlNotSupported(context.into()))
        }
        ffi::CameraResultFFI::EnumerationFailed => {
            Err(CameraError::EnumerationFailed(context.into()))
        }
        ffi::CameraResultFFI::NotFound => Err(CameraError::NotFound(context.into())),
        ffi::CameraResultFFI::OpenFailed => Err(CameraError::OpenFailed(context.into())),
        ffi::CameraResultFFI::StartFailed => Err(CameraError::StartFailed(context.into())),
        ffi::CameraResultFFI::CaptureFailed => Err(CameraError::CaptureFailed(context.into())),
        ffi::CameraResultFFI::PermissionDenied => Err(CameraError::PermissionDenied),
        ffi::CameraResultFFI::AlreadyInUse => Err(CameraError::AlreadyInUse),
    }
}

/// Internal frame data sent from Swift callback.
struct RawFrame {
    pixelbuffer_handle: u64,
    width: u32,
    height: u32,
    timestamp_ns: u64,
}

// Global channel for frame delivery (set when camera starts)
static FRAME_SENDER: std::sync::OnceLock<async_channel::Sender<RawFrame>> =
    std::sync::OnceLock::new();

/// Callback invoked from Swift for each camera frame.
extern "C" fn frame_callback(pixelbuffer_handle: u64, width: u32, height: u32, timestamp_ns: u64) {
    if let Some(sender) = FRAME_SENDER.get() {
        let frame = RawFrame {
            pixelbuffer_handle,
            width,
            height,
            timestamp_ns,
        };
        // Non-blocking send - drop frame if consumer is slow
        if sender.try_send(frame).is_err() {
            // Release the CVPixelBuffer since we're dropping this frame
            unsafe {
                camera_release_pixelbuffer(pixelbuffer_handle);
            }
        }
    } else {
        // No receiver, release immediately
        unsafe {
            camera_release_pixelbuffer(pixelbuffer_handle);
        }
    }
}

// Raw FFI for CVPixelBuffer (Core Video type)
#[allow(non_camel_case_types)]
type CVPixelBufferRef = *const std::ffi::c_void;
type CVReturn = i32;
const K_CV_RETURN_SUCCESS: CVReturn = 0;

unsafe extern "C" {
    fn CVPixelBufferLockBaseAddress(pixelBuffer: CVPixelBufferRef, lockFlags: u64) -> CVReturn;
    fn CVPixelBufferUnlockBaseAddress(pixelBuffer: CVPixelBufferRef, lockFlags: u64) -> CVReturn;
    fn CVPixelBufferGetBaseAddress(pixelBuffer: CVPixelBufferRef) -> *mut std::ffi::c_void;
    fn CVPixelBufferGetBytesPerRow(pixelBuffer: CVPixelBufferRef) -> usize;
    fn CVPixelBufferGetWidth(pixelBuffer: CVPixelBufferRef) -> usize;
    fn CVPixelBufferGetHeight(pixelBuffer: CVPixelBufferRef) -> usize;
}

// Lock flag for read-only access
const K_CV_PIXEL_BUFFER_LOCK_READ_ONLY: u64 = 0x00000001;

/// Create a wgpu texture from a `CVPixelBuffer` handle.
///
/// Copies pixel data from the pixel buffer to a GPU texture.
fn create_texture_from_pixelbuffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pixelbuffer_handle: u64,
    width: u32,
    height: u32,
) -> Result<wgpu::Texture, CameraError> {
    // Get the CVPixelBuffer pointer
    if pixelbuffer_handle == 0 {
        return Err(CameraError::GpuError("null CVPixelBuffer handle".into()));
    }

    // The handle is a raw pointer to the CVPixelBuffer (retained by Swift)
    let pixelbuffer = pixelbuffer_handle as CVPixelBufferRef;

    // Lock the CVPixelBuffer for reading
    let lock_result =
        unsafe { CVPixelBufferLockBaseAddress(pixelbuffer, K_CV_PIXEL_BUFFER_LOCK_READ_ONLY) };
    if lock_result != K_CV_RETURN_SUCCESS {
        return Err(CameraError::GpuError(format!(
            "failed to lock CVPixelBuffer: {lock_result}"
        )));
    }

    // Get CVPixelBuffer properties
    let base_address = unsafe { CVPixelBufferGetBaseAddress(pixelbuffer) };
    let bytes_per_row = unsafe { CVPixelBufferGetBytesPerRow(pixelbuffer) };
    #[allow(clippy::cast_possible_truncation)]
    let actual_width = unsafe { CVPixelBufferGetWidth(pixelbuffer) as u32 };
    #[allow(clippy::cast_possible_truncation)]
    let actual_height = unsafe { CVPixelBufferGetHeight(pixelbuffer) as u32 };

    // Use actual dimensions from CVPixelBuffer
    let width = actual_width.max(width);
    let height = actual_height.max(height);

    // Copy pixel data
    let data_size = bytes_per_row * height as usize;
    let pixel_data =
        unsafe { std::slice::from_raw_parts(base_address as *const u8, data_size) };

    // Create texture
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("CameraFrame"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });

    // Upload to GPU
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        pixel_data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            #[allow(clippy::cast_possible_truncation)]
            bytes_per_row: Some(bytes_per_row as u32),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );

    // Unlock CVPixelBuffer
    unsafe { CVPixelBufferUnlockBaseAddress(pixelbuffer, K_CV_PIXEL_BUFFER_LOCK_READ_ONLY) };

    Ok(texture)
}

/// Internal camera backend for Apple platforms.
pub struct CameraInner {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    capabilities: CameraCapabilities,
    controls: CameraControls,
    resolution: Resolution,
    frame_receiver: async_channel::Receiver<RawFrame>,
}

impl CameraInner {
    /// List available camera devices.
    pub fn list() -> Result<Vec<CameraInfo>, CameraError> {
        let count = ffi::camera_device_count();
        #[allow(clippy::cast_sign_loss)]
        let mut devices = Vec::with_capacity(count as usize);

        for i in 0..count {
            let id = ffi::camera_device_id(i);
            let name = ffi::camera_device_name(i);
            let description = ffi::camera_device_description(i);
            let is_front = ffi::camera_device_is_front(i);

            devices.push(CameraInfo {
                id,
                name,
                description: if description.is_empty() {
                    None
                } else {
                    Some(description)
                },
                is_front_facing: is_front,
            });
        }

        Ok(devices)
    }

    /// Open a camera by its ID.
    pub async fn open(
        camera_id: &str,
        config: CameraConfig,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
    ) -> Result<Self, CameraError> {
        convert_result(ffi::camera_open(camera_id.to_string()), camera_id)?;

        // Set resolution
        convert_result(
            ffi::camera_set_resolution(config.resolution.width, config.resolution.height),
            "set_resolution",
        )?;

        let w = ffi::camera_get_resolution_width();
        let h = ffi::camera_get_resolution_height();

        // Query capabilities
        let capabilities = Self::query_capabilities();

        // Create frame channel (bounded to prevent unbounded memory growth)
        let (sender, receiver) = async_channel::bounded(2);

        // Store sender globally for callback
        let _ = FRAME_SENDER.set(sender);

        // Set up frame callback
        unsafe {
            camera_set_frame_callback(frame_callback);
        }

        // Start streaming immediately (RAII)
        convert_result(ffi::camera_start(), "start")?;

        Ok(Self {
            device,
            queue,
            capabilities,
            controls: CameraControls::default(),
            resolution: Resolution {
                width: w,
                height: h,
            },
            frame_receiver: receiver,
        })
    }

    fn query_capabilities() -> CameraCapabilities {
        let iso_min = ffi::camera_get_iso_min();
        let iso_max = ffi::camera_get_iso_max();
        let exp_min_ns = ffi::camera_get_exposure_duration_min_ns();
        let exp_max_ns = ffi::camera_get_exposure_duration_max_ns();

        let mut stabilization_modes = vec![StabilizationMode::Off];
        if ffi::camera_supports_standard_stabilization() {
            stabilization_modes.push(StabilizationMode::Standard);
        }
        if ffi::camera_supports_cinematic_stabilization() {
            stabilization_modes.push(StabilizationMode::Cinematic);
        }

        CameraCapabilities {
            resolutions: vec![
                Resolution::UHD,
                Resolution::FULL_HD,
                Resolution::HD,
                Resolution { width: 640, height: 480 },
            ],
            frame_rates: vec![30, 60],
            iso_range: if iso_max > iso_min {
                Some((iso_min, iso_max))
            } else {
                None
            },
            exposure_duration_range: if exp_max_ns > exp_min_ns {
                Some((
                    Duration::from_nanos(exp_min_ns),
                    Duration::from_nanos(exp_max_ns),
                ))
            } else {
                None
            },
            supports_exposure_compensation: ffi::camera_supports_exposure_compensation(),
            supports_manual_focus: ffi::camera_supports_manual_focus(),
            supports_manual_white_balance: ffi::camera_supports_manual_white_balance(),
            zoom_range: {
                let min = ffi::camera_get_zoom_min();
                let max = ffi::camera_get_zoom_max();
                if max > min {
                    Some((min, max))
                } else {
                    None
                }
            },
            supports_hdr: ffi::camera_supports_hdr(),
            stabilization_modes,
            has_flash: ffi::camera_has_flash(),
            has_torch: ffi::camera_has_torch(),
        }
    }

    pub fn capabilities(&self) -> &CameraCapabilities {
        &self.capabilities
    }

    pub fn apply_controls(&mut self, controls: &CameraControls) -> Result<(), CameraError> {
        // Exposure
        if let Some(ref exposure) = controls.exposure {
            self.apply_exposure(exposure)?;
        }

        // Focus
        if let Some(ref focus) = controls.focus {
            self.apply_focus(focus)?;
        }

        // White balance
        if let Some(ref wb) = controls.white_balance {
            self.apply_white_balance(wb)?;
        }

        // Zoom
        if let Some(zoom) = controls.zoom {
            if self.capabilities.zoom_range.is_none() {
                return Err(CameraError::ControlNotSupported("zoom".into()));
            }
            convert_result(ffi::camera_set_zoom(zoom), "zoom")?;
            self.controls.zoom = Some(zoom);
        }

        // Flash
        if let Some(flash) = controls.flash {
            if !self.capabilities.has_flash && !self.capabilities.has_torch {
                return Err(CameraError::ControlNotSupported("flash".into()));
            }
            let mode = match flash {
                FlashMode::Off => 0,
                FlashMode::On => 1,
                FlashMode::Auto => 2,
                FlashMode::Torch => {
                    if !self.capabilities.has_torch {
                        return Err(CameraError::ControlNotSupported("torch".into()));
                    }
                    convert_result(ffi::camera_set_torch_mode(true), "torch")?;
                    self.controls.flash = Some(flash);
                    return Ok(());
                }
            };
            convert_result(ffi::camera_set_flash_mode(mode), "flash")?;
            // Turn off torch if switching away from it
            if self.controls.flash == Some(FlashMode::Torch) {
                let _ = ffi::camera_set_torch_mode(false);
            }
            self.controls.flash = Some(flash);
        }

        // HDR
        if let Some(hdr) = controls.hdr {
            if !self.capabilities.supports_hdr {
                return Err(CameraError::ControlNotSupported("hdr".into()));
            }
            convert_result(ffi::camera_set_hdr(hdr), "hdr")?;
            self.controls.hdr = Some(hdr);
        }

        // Stabilization
        if let Some(stabilization) = controls.stabilization {
            if !self.capabilities.stabilization_modes.contains(&stabilization) {
                return Err(CameraError::ControlNotSupported(format!(
                    "stabilization {stabilization:?}"
                )));
            }
            let mode = match stabilization {
                StabilizationMode::Off => 0,
                StabilizationMode::Standard => 1,
                StabilizationMode::Cinematic => 2,
            };
            convert_result(ffi::camera_set_stabilization_mode(mode), "stabilization")?;
            self.controls.stabilization = Some(stabilization);
        }

        Ok(())
    }

    fn apply_exposure(&mut self, exposure: &ExposureControl) -> Result<(), CameraError> {
        let mode = match exposure.mode {
            ExposureMode::Auto => 0,
            ExposureMode::Manual => 1,
            ExposureMode::Locked => 2,
        };
        convert_result(ffi::camera_set_exposure_mode(mode), "exposure_mode")?;

        if exposure.mode == ExposureMode::Manual {
            if let Some(iso) = exposure.iso {
                if let Some((min, max)) = self.capabilities.iso_range {
                    if iso < min || iso > max {
                        return Err(CameraError::ValueOutOfRange(format!(
                            "ISO {iso} not in range [{min}, {max}]"
                        )));
                    }
                } else {
                    return Err(CameraError::ControlNotSupported("iso".into()));
                }
                convert_result(ffi::camera_set_iso(iso), "iso")?;
            }

            if let Some(duration) = exposure.duration {
                if let Some((min, max)) = self.capabilities.exposure_duration_range {
                    if duration < min || duration > max {
                        return Err(CameraError::ValueOutOfRange(format!(
                            "exposure duration {duration:?} not in range [{min:?}, {max:?}]"
                        )));
                    }
                } else {
                    return Err(CameraError::ControlNotSupported("exposure_duration".into()));
                }
                #[allow(clippy::cast_possible_truncation)]
                let duration_ns = duration.as_nanos() as u64;
                convert_result(
                    ffi::camera_set_exposure_duration_ns(duration_ns),
                    "exposure_duration",
                )?;
            }
        }

        if let Some(ev) = exposure.compensation {
            if !self.capabilities.supports_exposure_compensation {
                return Err(CameraError::ControlNotSupported(
                    "exposure_compensation".into(),
                ));
            }
            convert_result(ffi::camera_set_exposure_compensation(ev), "exposure_compensation")?;
        }

        self.controls.exposure = Some(exposure.clone());
        Ok(())
    }

    fn apply_focus(&mut self, focus: &FocusControl) -> Result<(), CameraError> {
        let mode = match focus.mode {
            FocusMode::ContinuousAuto => 0,
            FocusMode::Auto => 1,
            FocusMode::Manual => 2,
            FocusMode::Locked => 3,
        };
        convert_result(ffi::camera_set_focus_mode(mode), "focus_mode")?;

        if focus.mode == FocusMode::Manual && focus.distance.is_some() {
            let distance = focus.distance.unwrap();
            if !self.capabilities.supports_manual_focus {
                return Err(CameraError::ControlNotSupported("manual_focus".into()));
            }
            if !(0.0..=1.0).contains(&distance) {
                return Err(CameraError::ValueOutOfRange(format!(
                    "focus distance {distance} not in range [0.0, 1.0]"
                )));
            }
            convert_result(ffi::camera_set_focus_distance(distance), "focus_distance")?;
        }

        if let Some((x, y)) = focus.point_of_interest {
            if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
                return Err(CameraError::ValueOutOfRange(
                    "focus point must be in range [0.0, 1.0]".into(),
                ));
            }
            convert_result(ffi::camera_set_focus_point(x, y), "focus_point")?;
        }

        self.controls.focus = Some(focus.clone());
        Ok(())
    }

    fn apply_white_balance(&mut self, wb: &WhiteBalanceControl) -> Result<(), CameraError> {
        let mode = match wb.mode {
            WhiteBalanceMode::Auto => 0,
            WhiteBalanceMode::Manual | WhiteBalanceMode::Daylight | WhiteBalanceMode::Cloudy
            | WhiteBalanceMode::Tungsten | WhiteBalanceMode::Fluorescent => 1,
        };
        convert_result(ffi::camera_set_white_balance_mode(mode), "white_balance_mode")?;

        // Set temperature for presets or manual
        let temperature = match wb.mode {
            WhiteBalanceMode::Auto => None,
            WhiteBalanceMode::Manual => wb.temperature,
            WhiteBalanceMode::Daylight => Some(5600),
            WhiteBalanceMode::Cloudy => Some(6500),
            WhiteBalanceMode::Tungsten => Some(3200),
            WhiteBalanceMode::Fluorescent => Some(4000),
        };

        if let Some(kelvin) = temperature {
            if !self.capabilities.supports_manual_white_balance {
                return Err(CameraError::ControlNotSupported("manual_white_balance".into()));
            }
            convert_result(
                ffi::camera_set_white_balance_temperature(kelvin),
                "white_balance_temperature",
            )?;
        }

        self.controls.white_balance = Some(wb.clone());
        Ok(())
    }

    pub fn controls(&self) -> &CameraControls {
        &self.controls
    }

    pub fn resolution(&self) -> Resolution {
        self.resolution
    }

    pub fn frames(&self) -> impl futures::Stream<Item = Frame> + '_ {
        let device = self.device.clone();
        let queue = self.queue.clone();
        let receiver = self.frame_receiver.clone();

        futures::stream::unfold(
            (device, queue, receiver),
            move |(device, queue, receiver)| async move {
                let raw_frame = receiver.recv().await.ok()?;

                // Create wgpu texture from CVPixelBuffer
                let texture = match create_texture_from_pixelbuffer(
                    &device,
                    &queue,
                    raw_frame.pixelbuffer_handle,
                    raw_frame.width,
                    raw_frame.height,
                ) {
                    Ok(tex) => tex,
                    Err(_) => {
                        // Release CVPixelBuffer on error
                        unsafe {
                            camera_release_pixelbuffer(raw_frame.pixelbuffer_handle);
                        }
                        // Continue to next frame
                        return Some((None, (device, queue, receiver)));
                    }
                };

                // Release the CVPixelBuffer now that we've created the texture
                unsafe {
                    camera_release_pixelbuffer(raw_frame.pixelbuffer_handle);
                }

                let frame = Frame {
                    texture,
                    width: raw_frame.width,
                    height: raw_frame.height,
                    format: PixelFormat::Bgra8,
                    timestamp: Duration::from_nanos(raw_frame.timestamp_ns),
                };

                Some((Some(frame), (device, queue, receiver)))
            },
        )
        .filter_map(|opt| async move { opt })
    }

    pub async fn capture_photo(&mut self) -> Result<Photo, CameraError> {
        convert_result(ffi::camera_take_photo(), "take_photo")?;

        let len = ffi::camera_get_photo_len();
        if len <= 0 {
            return Err(CameraError::CaptureFailed("empty photo data".into()));
        }

        #[allow(clippy::cast_sign_loss)]
        let mut data = vec![0u8; len as usize];
        unsafe {
            #[allow(clippy::cast_sign_loss)]
            camera_copy_photo_data(data.as_mut_ptr(), len as u64);
        }

        let width = self.resolution.width;
        let height = self.resolution.height;

        // Create GPU texture
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("CameraPhoto"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Upload to GPU
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        Ok(Photo {
            texture,
            width,
            height,
        })
    }

    pub fn start_recording(&mut self, path: &Path) -> Result<(), CameraError> {
        let path_str = path.to_string_lossy().to_string();
        convert_result(
            ffi::camera_start_recording(path_str),
            "start_recording",
        )
    }

    pub fn stop_recording(&mut self) -> Result<(), CameraError> {
        convert_result(ffi::camera_stop_recording(), "stop_recording")
    }

    pub fn recording_duration(&self) -> Duration {
        Duration::from_millis(ffi::camera_get_recording_duration_ms())
    }
}

impl Drop for CameraInner {
    fn drop(&mut self) {
        // Clear the frame callback
        unsafe {
            camera_clear_frame_callback();
        }
        // Stop the camera
        let _ = ffi::camera_stop();
    }
}
