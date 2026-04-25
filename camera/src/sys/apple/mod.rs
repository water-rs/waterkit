//! Apple platform (iOS/macOS) camera implementation using `AVCaptureSession`.
//!
//! Uses Metal texture interop for zero-copy frame rendering with wgpu.

use crate::{
    CameraCapabilities, CameraConfig, CameraControls, CameraError, CameraInfo, DynamicRangeProfile,
    ExposureControl, ExposureMode, FlashMode, FocusControl, FocusMode, Frame, Photo, PixelFormat,
    RawPhoto, RawPhotoFormat, RawVideoFormat, Resolution, StabilizationMode, WhiteBalanceControl,
    WhiteBalanceMode,
};
use futures::StreamExt;
use std::num::NonZeroU8;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordingMode {
    Standard,
    Raw,
}

#[swift_bridge::bridge]
mod ffi {
    enum CameraResultFFI {
        Success,
        Unsupported,
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
        fn camera_supports_dolby_vision() -> bool;
        fn camera_supports_standard_stabilization() -> bool;
        fn camera_supports_cinematic_stabilization() -> bool;
        fn camera_has_flash() -> bool;
        fn camera_has_torch() -> bool;
        fn camera_supports_concurrent_multicam() -> bool;
        fn camera_max_concurrent_cameras() -> u8;
        fn camera_supports_raw_photo() -> bool;
        fn camera_supports_raw_video() -> bool;

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
        fn camera_set_dynamic_range(profile: u8) -> CameraResultFFI;

        // Stabilization
        fn camera_set_stabilization_mode(mode: u8) -> CameraResultFFI;

        // Photo capture
        fn camera_take_photo() -> CameraResultFFI;
        fn camera_get_photo_len() -> i32;
        fn camera_take_raw_photo() -> CameraResultFFI;
        fn camera_get_raw_photo_len() -> i32;

        // Recording
        fn camera_start_recording(path: String) -> CameraResultFFI;
        fn camera_stop_recording() -> CameraResultFFI;
        fn camera_get_recording_duration_ms() -> u64;
        fn camera_start_raw_recording(path: String) -> CameraResultFFI;
        fn camera_stop_raw_recording() -> CameraResultFFI;
        fn camera_get_raw_recording_duration_ms() -> u64;
    }
}

// External C functions
unsafe extern "C" {
    fn camera_set_frame_callback(callback: extern "C" fn(u64, u32, u32, u64));
    fn camera_clear_frame_callback();
    fn camera_release_pixelbuffer(handle: u64);
    fn camera_copy_photo_data(buffer: *mut u8, size: u64);
    fn camera_copy_raw_photo_data(buffer: *mut u8, size: u64);
}

fn convert_result(result: ffi::CameraResultFFI, context: &str) -> Result<(), CameraError> {
    match result {
        ffi::CameraResultFFI::Success => Ok(()),
        ffi::CameraResultFFI::Unsupported => Err(CameraError::ControlUnsupported(context.into())),
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

// Global channel slot for frame delivery (updated when camera starts/stops)
static FRAME_SENDER: std::sync::OnceLock<Mutex<Option<async_channel::Sender<RawFrame>>>> =
    std::sync::OnceLock::new();

fn frame_sender_slot() -> &'static Mutex<Option<async_channel::Sender<RawFrame>>> {
    FRAME_SENDER.get_or_init(|| Mutex::new(None))
}

fn frame_sender_lock() -> std::sync::MutexGuard<'static, Option<async_channel::Sender<RawFrame>>> {
    frame_sender_slot()
        .lock()
        .unwrap_or_else(|_| std::process::abort())
}

/// Callback invoked from Swift for each camera frame.
extern "C" fn frame_callback(pixelbuffer_handle: u64, width: u32, height: u32, timestamp_ns: u64) {
    let sender = { frame_sender_lock().clone() };

    if let Some(sender) = sender {
        let frame = RawFrame {
            pixelbuffer_handle,
            width,
            height,
            timestamp_ns,
        };
        // Non-blocking send. force_send keeps latency low by replacing stale queued frames.
        match sender.force_send(frame) {
            Ok(Some(evicted)) => unsafe {
                camera_release_pixelbuffer(evicted.pixelbuffer_handle);
            },
            Ok(None) => {}
            Err(error) => unsafe {
                camera_release_pixelbuffer(error.0.pixelbuffer_handle);
            },
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
const K_CV_PIXEL_BUFFER_LOCK_READ_ONLY: u64 = 0x0000_0001;

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
    struct PixelBufferReadLockGuard(CVPixelBufferRef);

    impl Drop for PixelBufferReadLockGuard {
        fn drop(&mut self) {
            unsafe {
                CVPixelBufferUnlockBaseAddress(self.0, K_CV_PIXEL_BUFFER_LOCK_READ_ONLY);
            }
        }
    }

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
    let _lock_guard = PixelBufferReadLockGuard(pixelbuffer);

    // Get CVPixelBuffer properties
    let base_address = unsafe { CVPixelBufferGetBaseAddress(pixelbuffer) };
    if base_address.is_null() {
        return Err(CameraError::GpuError(
            "CVPixelBuffer base address is null".into(),
        ));
    }

    let bytes_per_row = unsafe { CVPixelBufferGetBytesPerRow(pixelbuffer) };
    #[allow(clippy::cast_possible_truncation)]
    let actual_width = unsafe { CVPixelBufferGetWidth(pixelbuffer) as u32 };
    #[allow(clippy::cast_possible_truncation)]
    let actual_height = unsafe { CVPixelBufferGetHeight(pixelbuffer) as u32 };

    let width = if actual_width > 0 {
        actual_width
    } else {
        width
    };
    let height = if actual_height > 0 {
        actual_height
    } else {
        height
    };
    if width == 0 || height == 0 {
        return Err(CameraError::GpuError(
            "CVPixelBuffer reported zero dimensions".into(),
        ));
    }

    let min_bytes_per_row = usize::try_from(width)
        .ok()
        .and_then(|w| w.checked_mul(4))
        .ok_or_else(|| CameraError::GpuError("frame width overflows row stride".into()))?;
    if bytes_per_row < min_bytes_per_row {
        return Err(CameraError::GpuError(format!(
            "CVPixelBuffer bytes_per_row {bytes_per_row} is smaller than minimum {min_bytes_per_row}"
        )));
    }

    // Copy pixel data
    let data_size = bytes_per_row
        .checked_mul(
            usize::try_from(height)
                .map_err(|_| CameraError::GpuError("frame height overflows usize".into()))?,
        )
        .ok_or_else(|| CameraError::GpuError("pixel buffer size overflows usize".into()))?;
    let pixel_data = unsafe { std::slice::from_raw_parts(base_address as *const u8, data_size) };

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
    recording_mode: Option<RecordingMode>,
}

impl CameraInner {
    /// List available camera devices.
    pub fn list() -> Result<Vec<CameraInfo>, CameraError> {
        let count = ffi::camera_device_count();
        let count = usize::try_from(count).map_err(|_| {
            CameraError::EnumerationFailed("Apple camera device count was negative".into())
        })?;
        let mut devices = Vec::with_capacity(count);

        for i in 0..count {
            let i =
                i32::try_from(i).expect("Apple camera device index originated from an i32 count");
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
        std::future::ready(()).await;
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
        capabilities.validate()?;

        // Create frame channel (bounded to prevent unbounded memory growth)
        let (sender, receiver) = async_channel::bounded(1);

        // Store sender for callback dispatch.
        {
            let mut guard = frame_sender_lock();
            if guard.is_some() {
                return Err(CameraError::AlreadyInUse);
            }
            *guard = Some(sender);
        }

        // Set up frame callback
        unsafe {
            camera_set_frame_callback(frame_callback);
        }

        // Start streaming immediately (RAII)
        if let Err(error) = convert_result(ffi::camera_start(), "start") {
            {
                let mut guard = frame_sender_lock();
                *guard = None;
            }
            unsafe {
                camera_clear_frame_callback();
            }
            let _ = ffi::camera_stop();
            return Err(error);
        }

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
            recording_mode: None,
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
                Resolution {
                    width: 640,
                    height: 480,
                },
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
                if max > min { Some((min, max)) } else { None }
            },
            dynamic_ranges: {
                let mut ranges = vec![DynamicRangeProfile::Sdr];
                if ffi::camera_supports_hdr() {
                    ranges.push(DynamicRangeProfile::Hdr10);
                    ranges.push(DynamicRangeProfile::Hlg10);
                }
                if ffi::camera_supports_dolby_vision() {
                    ranges.push(DynamicRangeProfile::DolbyVision);
                }
                ranges
            },
            supports_dolby_vision: ffi::camera_supports_dolby_vision(),
            stabilization_modes,
            has_flash: ffi::camera_has_flash(),
            has_torch: ffi::camera_has_torch(),
            supports_concurrent_multi_camera: false,
            max_concurrent_cameras: NonZeroU8::MIN,
            uses_system_photo_pipeline: true,
            uses_system_video_pipeline: true,
            supports_raw_photo: ffi::camera_supports_raw_photo(),
            raw_photo_formats: if ffi::camera_supports_raw_photo() {
                vec![RawPhotoFormat::Dng]
            } else {
                Vec::new()
            },
            supports_raw_video: ffi::camera_supports_raw_video(),
            raw_video_formats: if ffi::camera_supports_raw_video() {
                vec![RawVideoFormat::Bgra8Frames]
            } else {
                Vec::new()
            },
        }
    }

    pub const fn capabilities(&self) -> &CameraCapabilities {
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
                return Err(CameraError::ControlUnsupported("zoom".into()));
            }
            convert_result(ffi::camera_set_zoom(zoom), "zoom")?;
            self.controls.zoom = Some(zoom);
        }

        // Flash
        if let Some(flash) = controls.flash {
            if !self.capabilities.has_flash && !self.capabilities.has_torch {
                return Err(CameraError::ControlUnsupported("flash".into()));
            }
            let mode = match flash {
                FlashMode::Off => 0,
                FlashMode::On => 1,
                FlashMode::Auto => 2,
                FlashMode::Torch => {
                    if !self.capabilities.has_torch {
                        return Err(CameraError::ControlUnsupported("torch".into()));
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

        // Dynamic range
        if let Some(profile) = controls.dynamic_range {
            if !self.capabilities.dynamic_ranges.contains(&profile) {
                return Err(CameraError::ControlUnsupported(format!(
                    "dynamic_range.{profile:?}"
                )));
            }
            let mode = match profile {
                DynamicRangeProfile::Sdr => 0,
                DynamicRangeProfile::Hdr10 => 1,
                DynamicRangeProfile::Hlg10 => 2,
                DynamicRangeProfile::DolbyVision => 3,
            };
            convert_result(ffi::camera_set_dynamic_range(mode), "dynamic_range")?;
            self.controls.dynamic_range = Some(profile);
        }

        // Stabilization
        if let Some(stabilization) = controls.stabilization {
            if !self
                .capabilities
                .stabilization_modes
                .contains(&stabilization)
            {
                return Err(CameraError::ControlUnsupported(format!(
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
                    return Err(CameraError::ControlUnsupported("iso".into()));
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
                    return Err(CameraError::ControlUnsupported("exposure_duration".into()));
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
                return Err(CameraError::ControlUnsupported(
                    "exposure_compensation".into(),
                ));
            }
            convert_result(
                ffi::camera_set_exposure_compensation(ev),
                "exposure_compensation",
            )?;
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

        if let Some(distance) = focus.distance.filter(|_| focus.mode == FocusMode::Manual) {
            if !self.capabilities.supports_manual_focus {
                return Err(CameraError::ControlUnsupported("manual_focus".into()));
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
            WhiteBalanceMode::Manual
            | WhiteBalanceMode::Daylight
            | WhiteBalanceMode::Cloudy
            | WhiteBalanceMode::Tungsten
            | WhiteBalanceMode::Fluorescent => 1,
        };
        convert_result(
            ffi::camera_set_white_balance_mode(mode),
            "white_balance_mode",
        )?;

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
                return Err(CameraError::ControlUnsupported(
                    "manual_white_balance".into(),
                ));
            }
            convert_result(
                ffi::camera_set_white_balance_temperature(kelvin),
                "white_balance_temperature",
            )?;
        }

        self.controls.white_balance = Some(wb.clone());
        Ok(())
    }

    pub const fn controls(&self) -> &CameraControls {
        &self.controls
    }

    pub const fn resolution(&self) -> Resolution {
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
                let Ok(texture) = create_texture_from_pixelbuffer(
                    &device,
                    &queue,
                    raw_frame.pixelbuffer_handle,
                    raw_frame.width,
                    raw_frame.height,
                ) else {
                    // Release CVPixelBuffer on error
                    unsafe {
                        camera_release_pixelbuffer(raw_frame.pixelbuffer_handle);
                    }
                    // Continue to next frame
                    return Some((None, (device, queue, receiver)));
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

    pub async fn capture_photo(&self) -> Result<Photo, CameraError> {
        std::future::ready(()).await;
        convert_result(ffi::camera_take_photo(), "take_photo")?;

        let len = ffi::camera_get_photo_len();
        if len <= 0 {
            return Err(CameraError::CaptureFailed("empty photo data".into()));
        }

        #[allow(clippy::cast_sign_loss)]
        let mut encoded = vec![0u8; len as usize];
        unsafe {
            #[allow(clippy::cast_sign_loss)]
            camera_copy_photo_data(encoded.as_mut_ptr(), len as u64);
        }

        let dynamic = image::load_from_memory(&encoded).map_err(|error| {
            CameraError::CaptureFailed(format!("failed to decode captured photo: {error}"))
        })?;
        let rgba = dynamic.to_rgba8();
        let width = rgba.width();
        let height = rgba.height();
        let data = rgba.into_raw();

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
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
            data.as_slice(),
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

    pub async fn capture_raw_photo(&self) -> Result<RawPhoto, CameraError> {
        std::future::ready(()).await;
        if !self.capabilities.supports_raw_photo {
            return Err(CameraError::ControlUnsupported("raw_photo".into()));
        }
        convert_result(ffi::camera_take_raw_photo(), "take_raw_photo")?;

        let len = ffi::camera_get_raw_photo_len();
        if len <= 0 {
            return Err(CameraError::CaptureFailed("empty raw photo data".into()));
        }

        #[allow(clippy::cast_sign_loss)]
        let mut data = vec![0u8; len as usize];
        unsafe {
            #[allow(clippy::cast_sign_loss)]
            camera_copy_raw_photo_data(data.as_mut_ptr(), len as u64);
        }

        Ok(RawPhoto {
            data,
            width: self.resolution.width,
            height: self.resolution.height,
            format: RawPhotoFormat::Dng,
        })
    }

    pub fn start_recording(&mut self, path: &Path) -> Result<(), CameraError> {
        if self.recording_mode.is_some() {
            return Err(CameraError::AlreadyInUse);
        }
        let path_str = path.to_string_lossy().to_string();
        convert_result(ffi::camera_start_recording(path_str), "start_recording")?;
        self.recording_mode = Some(RecordingMode::Standard);
        Ok(())
    }

    pub fn stop_recording(&mut self) -> Result<(), CameraError> {
        match self.recording_mode {
            Some(RecordingMode::Standard) => {
                convert_result(ffi::camera_stop_recording(), "stop_recording")?;
                self.recording_mode = None;
                Ok(())
            }
            Some(RecordingMode::Raw) => Err(CameraError::RecordingError(
                "raw recording active; call stop_raw_recording".into(),
            )),
            None => Ok(()),
        }
    }

    pub fn recording_duration(&self) -> Duration {
        match self.recording_mode {
            Some(RecordingMode::Standard) => {
                Duration::from_millis(ffi::camera_get_recording_duration_ms())
            }
            _ => Duration::ZERO,
        }
    }

    pub fn start_raw_recording(&mut self, path: &Path) -> Result<(), CameraError> {
        if !self.capabilities.supports_raw_video {
            return Err(CameraError::ControlUnsupported("raw_video".into()));
        }
        if self.recording_mode.is_some() {
            return Err(CameraError::AlreadyInUse);
        }
        let path_str = path.to_string_lossy().to_string();
        convert_result(
            ffi::camera_start_raw_recording(path_str),
            "start_raw_recording",
        )?;
        self.recording_mode = Some(RecordingMode::Raw);
        Ok(())
    }

    pub fn stop_raw_recording(&mut self) -> Result<(), CameraError> {
        match self.recording_mode {
            Some(RecordingMode::Raw) => {
                convert_result(ffi::camera_stop_raw_recording(), "stop_raw_recording")?;
                self.recording_mode = None;
                Ok(())
            }
            Some(RecordingMode::Standard) => Err(CameraError::RecordingError(
                "standard recording active; call stop_recording".into(),
            )),
            None => Ok(()),
        }
    }

    pub fn raw_recording_duration(&self) -> Duration {
        match self.recording_mode {
            Some(RecordingMode::Raw) => {
                Duration::from_millis(ffi::camera_get_raw_recording_duration_ms())
            }
            _ => Duration::ZERO,
        }
    }
}

impl Drop for CameraInner {
    fn drop(&mut self) {
        match self.recording_mode {
            Some(RecordingMode::Standard) => {
                let _ = ffi::camera_stop_recording();
            }
            Some(RecordingMode::Raw) => {
                let _ = ffi::camera_stop_raw_recording();
            }
            None => {}
        }
        {
            let mut guard = frame_sender_lock();
            *guard = None;
        }
        // Clear the frame callback
        unsafe {
            camera_clear_frame_callback();
        }
        // Stop the camera
        let _ = ffi::camera_stop();
    }
}
