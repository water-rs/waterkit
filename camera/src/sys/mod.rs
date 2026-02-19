//! Platform-specific camera implementations.

#[cfg(any(target_os = "ios", target_os = "macos"))]
pub mod apple;

#[cfg(target_os = "android")]
pub mod android;

#[cfg(any(target_os = "windows", target_os = "linux"))]
pub mod desktop;

// Apple platforms
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use apple::CameraInner;

// Android
#[cfg(target_os = "android")]
pub use android::CameraInner;

// Desktop (Windows, Linux) - use nokhwa
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub use desktop::CameraInner;

// Fallback for unsupported platforms
#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
mod fallback {
    use crate::{
        CameraCapabilities, CameraConfig, CameraControls, CameraError, CameraInfo, Frame, Photo,
        RawPhoto, Resolution,
    };
    use std::num::NonZeroU8;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    pub struct CameraInner;

    impl CameraInner {
        pub fn list() -> Result<Vec<CameraInfo>, CameraError> {
            Err(CameraError::NotSupported)
        }

        pub async fn open(
            _camera_id: &str,
            _config: CameraConfig,
            _device: Arc<wgpu::Device>,
            _queue: Arc<wgpu::Queue>,
        ) -> Result<Self, CameraError> {
            Err(CameraError::NotSupported)
        }

        pub fn capabilities(&self) -> &CameraCapabilities {
            static EMPTY: CameraCapabilities = CameraCapabilities {
                resolutions: Vec::new(),
                frame_rates: Vec::new(),
                iso_range: None,
                exposure_duration_range: None,
                supports_exposure_compensation: false,
                supports_manual_focus: false,
                supports_manual_white_balance: false,
                zoom_range: None,
                dynamic_ranges: Vec::new(),
                supports_dolby_vision: false,
                stabilization_modes: Vec::new(),
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
            };
            &EMPTY
        }

        pub fn apply_controls(&mut self, _controls: &CameraControls) -> Result<(), CameraError> {
            Err(CameraError::NotSupported)
        }

        pub fn controls(&self) -> &CameraControls {
            static EMPTY: CameraControls = CameraControls {
                exposure: None,
                focus: None,
                white_balance: None,
                zoom: None,
                flash: None,
                dynamic_range: None,
                stabilization: None,
            };
            &EMPTY
        }

        pub fn resolution(&self) -> Resolution {
            Resolution::HD
        }

        pub fn frames(&self) -> impl futures::Stream<Item = Frame> + '_ {
            futures::stream::empty()
        }

        pub async fn capture_photo(&self) -> Result<Photo, CameraError> {
            Err(CameraError::NotSupported)
        }

        pub async fn capture_raw_photo(&self) -> Result<RawPhoto, CameraError> {
            Err(CameraError::NotSupported)
        }

        pub fn start_recording(&mut self, _path: &Path) -> Result<(), CameraError> {
            Err(CameraError::NotSupported)
        }

        pub fn stop_recording(&mut self) -> Result<(), CameraError> {
            Err(CameraError::NotSupported)
        }

        pub fn recording_duration(&self) -> Duration {
            Duration::ZERO
        }

        pub fn start_raw_recording(&mut self, _path: &Path) -> Result<(), CameraError> {
            Err(CameraError::NotSupported)
        }

        pub fn stop_raw_recording(&mut self) -> Result<(), CameraError> {
            Err(CameraError::NotSupported)
        }

        pub fn raw_recording_duration(&self) -> Duration {
            Duration::ZERO
        }
    }
}

#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
pub use fallback::CameraInner;
