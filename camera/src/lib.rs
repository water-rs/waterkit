//! Cross-platform camera streaming with GPU-first frame delivery.
//!
//! This crate provides a unified API for camera enumeration and streaming
//! across iOS, macOS, Android, Windows, and Linux platforms. Frames are
//! delivered as GPU textures for zero-copy rendering.
//!
//! The camera API is fully RAII-based: cameras start streaming when opened
//! and stop when dropped.

#![allow(clippy::missing_const_for_fn)] // Platform implementations vary
//!
//! # Example
//!
//! ```ignore
//! use waterkit_camera::{Camera, CameraConfig};
//! use futures::StreamExt;
//!
//! async fn example(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) {
//!     // Camera starts streaming immediately on open
//!     let camera = Camera::open_default(device, queue).await.unwrap();
//!
//!     let mut frames = camera.frames();
//!     while let Some(frame) = frames.next().await {
//!         let view = frame.view();
//!         // Use texture view for rendering...
//!     }
//!     // Camera stops when dropped
//! }
//! ```

#![warn(missing_docs)]

mod sys;

use std::num::NonZeroU8;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

// Re-export wgpu types for convenience
pub use wgpu;

// ============================================================================
// Pixel Format
// ============================================================================

/// Pixel format for camera frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PixelFormat {
    /// BGRA 8-bit per channel (native on Apple platforms).
    #[default]
    Bgra8,
    /// RGBA 8-bit per channel.
    Rgba8,
    /// NV12 (YUV 4:2:0 bi-planar) - common camera format.
    Nv12,
}

impl PixelFormat {
    /// Get the corresponding wgpu texture format.
    ///
    /// For YUV formats, returns the format for the combined texture.
    /// Use a shader to convert YUV to RGB.
    #[must_use]
    #[allow(clippy::match_same_arms)] // Nv12 converts to Bgra8, intentionally same
    pub const fn wgpu_format(&self) -> wgpu::TextureFormat {
        match self {
            Self::Bgra8 | Self::Nv12 => wgpu::TextureFormat::Bgra8UnormSrgb,
            Self::Rgba8 => wgpu::TextureFormat::Rgba8UnormSrgb,
        }
    }

    /// Bytes per pixel for the format.
    #[must_use]
    pub const fn bytes_per_pixel(&self) -> u32 {
        match self {
            Self::Bgra8 | Self::Rgba8 => 4,
            Self::Nv12 => 1, // 1.5 average, but luma plane is 1 bpp
        }
    }
}

// ============================================================================
// Frame
// ============================================================================

/// A GPU-backed camera frame.
///
/// Contains a wgpu texture that can be used directly for rendering.
/// On supported platforms (Apple), this is zero-copy from the camera hardware.
pub struct Frame {
    texture: wgpu::Texture,
    width: u32,
    height: u32,
    format: PixelFormat,
    timestamp: Duration,
}

impl Frame {
    /// Get the underlying wgpu texture.
    #[must_use]
    pub const fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// Consume the frame and return the underlying texture.
    #[must_use]
    pub fn into_texture(self) -> wgpu::Texture {
        self.texture
    }

    /// Create a texture view for rendering.
    #[must_use]
    pub fn view(&self) -> wgpu::TextureView {
        self.texture
            .create_view(&wgpu::TextureViewDescriptor::default())
    }

    /// Frame width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Frame height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Pixel format.
    #[must_use]
    pub const fn format(&self) -> PixelFormat {
        self.format
    }

    /// Presentation timestamp since camera start.
    #[must_use]
    pub const fn timestamp(&self) -> Duration {
        self.timestamp
    }
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.format)
            .field("timestamp", &self.timestamp)
            .finish_non_exhaustive()
    }
}

// ============================================================================
// Resolution
// ============================================================================

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

impl Default for Resolution {
    fn default() -> Self {
        Self::FULL_HD
    }
}

// ============================================================================
// Camera Configuration
// ============================================================================

/// Camera configuration for initialization.
#[derive(Debug, Clone)]
pub struct CameraConfig {
    /// Desired resolution (actual may differ based on capabilities).
    pub resolution: Resolution,
    /// Desired frame rate.
    pub frame_rate: u32,
    /// Pixel format preference.
    pub format: PixelFormat,
}

impl Default for CameraConfig {
    fn default() -> Self {
        Self {
            resolution: Resolution::FULL_HD,
            frame_rate: 30,
            format: PixelFormat::Bgra8,
        }
    }
}

impl CameraConfig {
    /// Create a 4K configuration.
    #[must_use]
    pub fn uhd() -> Self {
        Self {
            resolution: Resolution::UHD,
            frame_rate: 30,
            ..Default::default()
        }
    }

    /// Create a high frame rate configuration (720p60).
    #[must_use]
    pub fn high_fps() -> Self {
        Self {
            resolution: Resolution::HD,
            frame_rate: 60,
            ..Default::default()
        }
    }
}

// ============================================================================
// Professional Camera Controls
// ============================================================================

/// Exposure mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ExposureMode {
    /// Automatic exposure.
    #[default]
    Auto,
    /// Manual exposure (user sets ISO and duration).
    Manual,
    /// Lock current exposure values.
    Locked,
}

/// Exposure control settings.
#[derive(Debug, Clone, Default)]
pub struct ExposureControl {
    /// Exposure mode.
    pub mode: ExposureMode,
    /// ISO sensitivity (e.g., 100-6400). Only used in Manual mode.
    pub iso: Option<f32>,
    /// Exposure duration (shutter speed). Only used in Manual mode.
    pub duration: Option<Duration>,
    /// Exposure compensation in EV (e.g., -3.0 to +3.0). Used in Auto mode.
    pub compensation: Option<f32>,
}

/// Focus mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FocusMode {
    /// Continuous auto-focus.
    #[default]
    ContinuousAuto,
    /// One-shot auto-focus.
    Auto,
    /// Manual focus.
    Manual,
    /// Lock current focus.
    Locked,
}

/// Focus control settings.
#[derive(Debug, Clone, Default)]
pub struct FocusControl {
    /// Focus mode.
    pub mode: FocusMode,
    /// Focus distance (0.0 = near, 1.0 = infinity). Only used in Manual mode.
    pub distance: Option<f32>,
    /// Point of interest for auto-focus (normalized 0-1 coordinates).
    pub point_of_interest: Option<(f32, f32)>,
}

/// White balance mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WhiteBalanceMode {
    /// Automatic white balance.
    #[default]
    Auto,
    /// Manual white balance (user sets temperature).
    Manual,
    /// Daylight preset (~5600K).
    Daylight,
    /// Cloudy preset (~6500K).
    Cloudy,
    /// Tungsten/incandescent preset (~3200K).
    Tungsten,
    /// Fluorescent preset (~4000K).
    Fluorescent,
}

/// White balance control settings.
#[derive(Debug, Clone, Default)]
pub struct WhiteBalanceControl {
    /// White balance mode.
    pub mode: WhiteBalanceMode,
    /// Color temperature in Kelvin (e.g., 2000-10000). Only used in Manual mode.
    pub temperature: Option<u32>,
}

/// Flash mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FlashMode {
    /// Flash off.
    #[default]
    Off,
    /// Flash on for photo capture.
    On,
    /// Automatic flash.
    Auto,
    /// Continuous torch light.
    Torch,
}

/// Video stabilization mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum StabilizationMode {
    /// No stabilization.
    #[default]
    Off,
    /// Standard stabilization.
    Standard,
    /// Cinematic stabilization (higher quality, more latency).
    Cinematic,
}

/// Dynamic range profile for photo/video capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DynamicRangeProfile {
    /// Standard dynamic range.
    #[default]
    Sdr,
    /// HDR10 dynamic range.
    Hdr10,
    /// HLG10 dynamic range.
    Hlg10,
    /// Dolby Vision dynamic range.
    DolbyVision,
}

/// RAW photo encoding format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RawPhotoFormat {
    /// Digital Negative (DNG) container with sensor data.
    #[default]
    Dng,
}

/// RAW video frame stream format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RawVideoFormat {
    /// Frame stream where each frame is BGRA8 pixels.
    #[default]
    Bgra8Frames,
    /// Frame stream where each frame is RGBA8 pixels.
    Rgba8Frames,
}

/// Aggregated camera controls.
///
/// Only `Some` values will be applied when passed to [`Camera::apply_controls`].
#[derive(Debug, Clone, Default)]
pub struct CameraControls {
    /// Exposure settings.
    pub exposure: Option<ExposureControl>,
    /// Focus settings.
    pub focus: Option<FocusControl>,
    /// White balance settings.
    pub white_balance: Option<WhiteBalanceControl>,
    /// Zoom factor (1.0 = no zoom).
    pub zoom: Option<f32>,
    /// Flash mode.
    pub flash: Option<FlashMode>,
    /// Dynamic range profile for preview/photo/video capture.
    pub dynamic_range: Option<DynamicRangeProfile>,
    /// Video stabilization.
    pub stabilization: Option<StabilizationMode>,
}

// ============================================================================
// Camera Capabilities
// ============================================================================

/// Camera capabilities - query what controls are supported.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct CameraCapabilities {
    /// Supported resolutions.
    pub resolutions: Vec<Resolution>,
    /// Supported frame rates.
    pub frame_rates: Vec<u32>,
    /// ISO range (min, max) or None if not supported.
    pub iso_range: Option<(f32, f32)>,
    /// Exposure duration range (min, max) or None if not supported.
    pub exposure_duration_range: Option<(Duration, Duration)>,
    /// Whether exposure compensation is supported.
    pub supports_exposure_compensation: bool,
    /// Whether manual focus is supported.
    pub supports_manual_focus: bool,
    /// Whether manual white balance is supported.
    pub supports_manual_white_balance: bool,
    /// Zoom range (min, max).
    pub zoom_range: Option<(f32, f32)>,
    /// Supported dynamic range profiles.
    pub dynamic_ranges: Vec<DynamicRangeProfile>,
    /// Whether Dolby Vision is supported.
    pub supports_dolby_vision: bool,
    /// Available stabilization modes.
    pub stabilization_modes: Vec<StabilizationMode>,
    /// Whether flash is available.
    pub has_flash: bool,
    /// Whether torch (continuous light) is available.
    pub has_torch: bool,
    /// Whether concurrent multi-camera streaming is supported.
    pub supports_concurrent_multi_camera: bool,
    /// Maximum number of cameras that can stream concurrently.
    pub max_concurrent_cameras: NonZeroU8,
    /// Whether still photo capture uses the platform-native photography pipeline.
    pub uses_system_photo_pipeline: bool,
    /// Whether video recording uses the platform-native capture pipeline.
    pub uses_system_video_pipeline: bool,
    /// Whether RAW photo capture is supported.
    pub supports_raw_photo: bool,
    /// Supported RAW photo formats.
    pub raw_photo_formats: Vec<RawPhotoFormat>,
    /// Whether RAW video capture is supported.
    pub supports_raw_video: bool,
    /// Supported RAW video stream formats.
    pub raw_video_formats: Vec<RawVideoFormat>,
}

impl Default for CameraCapabilities {
    fn default() -> Self {
        Self {
            resolutions: Vec::new(),
            frame_rates: Vec::new(),
            iso_range: None,
            exposure_duration_range: None,
            supports_exposure_compensation: false,
            supports_manual_focus: false,
            supports_manual_white_balance: false,
            zoom_range: None,
            dynamic_ranges: vec![DynamicRangeProfile::Sdr],
            supports_dolby_vision: false,
            stabilization_modes: vec![StabilizationMode::Off],
            has_flash: false,
            has_torch: false,
            supports_concurrent_multi_camera: false,
            max_concurrent_cameras: NonZeroU8::MIN,
            uses_system_photo_pipeline: false,
            uses_system_video_pipeline: false,
            supports_raw_photo: false,
            raw_photo_formats: Vec::new(),
            supports_raw_video: false,
            raw_video_formats: Vec::new(),
        }
    }
}

impl CameraCapabilities {
    pub(crate) fn validate(&self) -> Result<(), CameraError> {
        if self.dynamic_ranges.is_empty() {
            return Err(CameraError::PlatformError(
                "camera capabilities must include at least one dynamic range profile".into(),
            ));
        }
        if !self.dynamic_ranges.contains(&DynamicRangeProfile::Sdr) {
            return Err(CameraError::PlatformError(
                "camera capabilities must always include SDR dynamic range profile".into(),
            ));
        }
        if self.supports_dolby_vision
            && !self
                .dynamic_ranges
                .contains(&DynamicRangeProfile::DolbyVision)
        {
            return Err(CameraError::PlatformError(
                "supports_dolby_vision=true requires DolbyVision profile in dynamic_ranges".into(),
            ));
        }
        if self.supports_raw_photo && self.raw_photo_formats.is_empty() {
            return Err(CameraError::PlatformError(
                "supports_raw_photo=true requires non-empty raw_photo_formats".into(),
            ));
        }
        if !self.supports_raw_photo && !self.raw_photo_formats.is_empty() {
            return Err(CameraError::PlatformError(
                "supports_raw_photo=false requires empty raw_photo_formats".into(),
            ));
        }
        if self.supports_raw_video && self.raw_video_formats.is_empty() {
            return Err(CameraError::PlatformError(
                "supports_raw_video=true requires non-empty raw_video_formats".into(),
            ));
        }
        if !self.supports_raw_video && !self.raw_video_formats.is_empty() {
            return Err(CameraError::PlatformError(
                "supports_raw_video=false requires empty raw_video_formats".into(),
            ));
        }
        if self.supports_concurrent_multi_camera && self.max_concurrent_cameras.get() < 2 {
            return Err(CameraError::PlatformError(
                "supports_concurrent_multi_camera=true requires max_concurrent_cameras >= 2".into(),
            ));
        }
        if !self.supports_concurrent_multi_camera && self.max_concurrent_cameras.get() != 1 {
            return Err(CameraError::PlatformError(
                "supports_concurrent_multi_camera=false requires max_concurrent_cameras == 1"
                    .into(),
            ));
        }
        Ok(())
    }
}

// ============================================================================
// Camera Info
// ============================================================================

/// Information about a camera device.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CameraInfo {
    /// Unique identifier.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Whether the camera is front-facing.
    pub is_front_facing: bool,
}

// ============================================================================
// Photo
// ============================================================================

/// A high-quality captured photo as a GPU texture.
///
/// On mobile platforms, this uses computational photography pipelines
/// (Smart HDR, Night mode, etc.) for the highest quality capture.
pub struct Photo {
    texture: wgpu::Texture,
    width: u32,
    height: u32,
}

/// A captured RAW photo payload.
#[derive(Debug, Clone)]
pub struct RawPhoto {
    data: Vec<u8>,
    width: u32,
    height: u32,
    format: RawPhotoFormat,
}

impl RawPhoto {
    /// RAW photo bytes.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        self.data.as_slice()
    }

    /// Consume and return RAW bytes.
    #[must_use]
    pub fn into_data(self) -> Vec<u8> {
        self.data
    }

    /// Photo width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Photo height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// RAW encoding format.
    #[must_use]
    pub const fn format(&self) -> RawPhotoFormat {
        self.format
    }
}

impl Photo {
    /// Get the underlying wgpu texture.
    #[must_use]
    pub const fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// Create a texture view for rendering.
    #[must_use]
    pub fn view(&self) -> wgpu::TextureView {
        self.texture
            .create_view(&wgpu::TextureViewDescriptor::default())
    }

    /// Photo width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Photo height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }
}

impl std::fmt::Debug for Photo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Photo")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

// ============================================================================
// Recording (RAII)
// ============================================================================

/// RAII guard for video recording.
///
/// Recording stops automatically when this guard is dropped.
/// Call [`Recording::stop`] to explicitly stop and get any errors.
#[derive(Debug)]
pub struct Recording<'a> {
    camera: &'a mut Camera,
    stopped: bool,
}

impl Recording<'_> {
    /// Stop recording and finalize the file.
    ///
    /// # Errors
    /// Returns an error if recording cannot be stopped.
    pub fn stop(mut self) -> Result<(), CameraError> {
        self.stopped = true;
        self.camera.inner.stop_recording()
    }

    /// Get the recording duration so far.
    #[must_use]
    pub fn duration(&self) -> Duration {
        self.camera.inner.recording_duration()
    }
}

impl Drop for Recording<'_> {
    fn drop(&mut self) {
        if !self.stopped {
            let _ = self.camera.inner.stop_recording();
        }
    }
}

/// RAII guard for RAW video recording.
#[derive(Debug)]
pub struct RawRecording<'a> {
    camera: &'a mut Camera,
    stopped: bool,
}

impl RawRecording<'_> {
    /// Stop RAW recording and finalize the file.
    ///
    /// # Errors
    /// Returns an error if RAW recording cannot be stopped.
    pub fn stop(mut self) -> Result<(), CameraError> {
        self.stopped = true;
        self.camera.inner.stop_raw_recording()
    }

    /// Get the RAW recording duration so far.
    #[must_use]
    pub fn duration(&self) -> Duration {
        self.camera.inner.raw_recording_duration()
    }
}

impl Drop for RawRecording<'_> {
    fn drop(&mut self) {
        if !self.stopped {
            let _ = self.camera.inner.stop_raw_recording();
        }
    }
}

// ============================================================================
// Error
// ============================================================================

/// Errors that can occur with camera operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum CameraError {
    /// Camera is not supported on this platform.
    #[error("camera not supported on this platform")]
    Unsupported,
    /// Failed to enumerate cameras.
    #[error("failed to enumerate cameras: {0}")]
    EnumerationFailed(String),
    /// Camera not found.
    #[error("camera not found: {0}")]
    NotFound(String),
    /// Failed to open camera.
    #[error("failed to open camera: {0}")]
    OpenFailed(String),
    /// Failed to start camera.
    #[error("failed to start camera: {0}")]
    StartFailed(String),
    /// Failed to capture frame or photo.
    #[error("failed to capture: {0}")]
    CaptureFailed(String),
    /// Permission denied.
    #[error("camera permission denied")]
    PermissionDenied,
    /// Camera is already in use.
    #[error("camera is already in use")]
    AlreadyInUse,
    /// The requested control is not supported.
    #[error("control not supported: {0}")]
    ControlUnsupported(String),
    /// The requested value is out of range.
    #[error("value out of range: {0}")]
    ValueOutOfRange(String),
    /// GPU error.
    #[error("GPU error: {0}")]
    GpuError(String),
    /// Recording error.
    #[error("recording error: {0}")]
    RecordingError(String),
    /// Platform-specific error.
    #[error("platform error: {0}")]
    PlatformError(String),
}

// ============================================================================
// Camera
// ============================================================================

/// Async camera controller with GPU-first frame delivery.
pub struct Camera {
    inner: sys::CameraInner,
}

impl std::fmt::Debug for Camera {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Camera").finish_non_exhaustive()
    }
}

impl Camera {
    /// List available cameras on the system.
    ///
    /// # Errors
    /// Returns [`CameraError::EnumerationFailed`] if camera enumeration fails.
    pub fn list() -> Result<Vec<CameraInfo>, CameraError> {
        sys::CameraInner::list()
    }

    /// Open a camera by its ID with configuration and GPU device.
    ///
    /// The camera owns references to the wgpu device and queue for creating
    /// GPU textures from camera frames.
    ///
    /// # Errors
    /// Returns [`CameraError::OpenFailed`] if the camera cannot be opened.
    pub async fn open(
        camera_id: &str,
        config: CameraConfig,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
    ) -> Result<Self, CameraError> {
        Ok(Self {
            inner: sys::CameraInner::open(camera_id, config, device, queue).await?,
        })
    }

    /// Open the default camera with default configuration.
    ///
    /// On desktop, this is typically the first webcam.
    /// On mobile, this is typically the back camera.
    ///
    /// # Errors
    /// Returns [`CameraError::NotFound`] if no camera is available.
    pub async fn open_default(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
    ) -> Result<Self, CameraError> {
        let cameras = Self::list()?;
        let camera = cameras
            .first()
            .ok_or_else(|| CameraError::NotFound("no cameras available".into()))?;
        Self::open(&camera.id, CameraConfig::default(), device, queue).await
    }

    /// Get camera capabilities.
    #[must_use]
    pub fn capabilities(&self) -> &CameraCapabilities {
        self.inner.capabilities()
    }

    /// Apply camera controls.
    ///
    /// Only fields that are `Some` will be modified. Returns an error if
    /// any control is not supported on this camera/platform.
    ///
    /// # Errors
    /// Returns [`CameraError::ControlUnsupported`] if a control is not available.
    /// Returns [`CameraError::ValueOutOfRange`] if a value is outside the supported range.
    pub fn apply_controls(&mut self, controls: &CameraControls) -> Result<(), CameraError> {
        self.inner.apply_controls(controls)
    }

    /// Get the current control values.
    #[must_use]
    pub fn controls(&self) -> &CameraControls {
        self.inner.controls()
    }

    /// Get the current resolution.
    #[must_use]
    pub fn resolution(&self) -> Resolution {
        self.inner.resolution()
    }

    /// Get an async stream of GPU-backed frames.
    ///
    /// Frames are delivered at the camera's frame rate. The stream implements
    /// backpressure - if frames are not consumed fast enough, older frames
    /// will be dropped.
    pub fn frames(&self) -> impl futures::Stream<Item = Frame> + '_ {
        self.inner.frames()
    }

    /// Capture a high-quality photo.
    ///
    /// On mobile, this uses computational photography pipelines (Smart HDR, etc).
    /// Returns the photo as JPEG data.
    ///
    /// # Errors
    /// Returns [`CameraError::CaptureFailed`] if the photo cannot be captured.
    pub async fn capture_photo(&mut self) -> Result<Photo, CameraError> {
        self.inner.capture_photo().await
    }

    /// Capture a RAW photo.
    ///
    /// Returns RAW bytes in a platform-supported format (typically DNG).
    ///
    /// # Errors
    /// Returns [`CameraError::CaptureFailed`] if RAW photo capture fails.
    /// Returns [`CameraError::ControlUnsupported`] if RAW photo is unsupported.
    pub async fn capture_raw_photo(&mut self) -> Result<RawPhoto, CameraError> {
        self.inner.capture_raw_photo().await
    }

    /// Start video recording with RAII guard.
    ///
    /// Recording stops automatically when the returned guard is dropped.
    ///
    /// # Errors
    /// Returns [`CameraError::RecordingError`] if recording cannot be started.
    pub fn recording(&mut self, path: impl AsRef<Path>) -> Result<Recording<'_>, CameraError> {
        self.inner.start_recording(path.as_ref())?;
        Ok(Recording {
            camera: self,
            stopped: false,
        })
    }

    /// Start RAW video recording with RAII guard.
    ///
    /// RAW video is written as an uncompressed frame stream file.
    ///
    /// # Errors
    /// Returns [`CameraError::RecordingError`] if RAW recording cannot be started.
    pub fn raw_recording(
        &mut self,
        path: impl AsRef<Path>,
    ) -> Result<RawRecording<'_>, CameraError> {
        self.inner.start_raw_recording(path.as_ref())?;
        Ok(RawRecording {
            camera: self,
            stopped: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_capabilities_satisfy_invariants() {
        CameraCapabilities::default()
            .validate()
            .expect("default capabilities must be valid");
    }

    #[test]
    fn raw_photo_support_requires_formats() {
        let capabilities = CameraCapabilities {
            supports_raw_photo: true,
            ..CameraCapabilities::default()
        };
        let error = capabilities
            .validate()
            .expect_err("raw photo without formats must fail");
        assert!(
            error
                .to_string()
                .contains("supports_raw_photo=true requires non-empty raw_photo_formats")
        );
    }

    #[test]
    fn concurrent_multi_camera_requires_capacity() {
        let capabilities = CameraCapabilities {
            supports_concurrent_multi_camera: true,
            max_concurrent_cameras: NonZeroU8::MIN,
            ..CameraCapabilities::default()
        };
        let error = capabilities
            .validate()
            .expect_err("invalid concurrent camera limit must fail");
        assert!(error.to_string().contains(
            "supports_concurrent_multi_camera=true requires max_concurrent_cameras >= 2"
        ));
    }
}
