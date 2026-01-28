//! Android camera implementation.
//!
//! Note: Full Camera2 API integration with GPU textures is pending.
//! Currently returns `NotSupported` for most operations.

#![allow(clippy::unused_async)]
#![allow(clippy::unused_self)]
#![allow(clippy::needless_pass_by_ref_mut)] // API consistency

use crate::{
    CameraCapabilities, CameraConfig, CameraControls, CameraError, CameraInfo, Frame, Photo,
    Resolution,
};
use std::sync::Arc;

/// Camera inner implementation for Android.
pub struct CameraInner {
    #[allow(dead_code)]
    device: Arc<wgpu::Device>,
    #[allow(dead_code)]
    queue: Arc<wgpu::Queue>,
    capabilities: CameraCapabilities,
    controls: CameraControls,
    resolution: Resolution,
}

impl std::fmt::Debug for CameraInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CameraInner")
            .field("resolution", &self.resolution)
            .finish_non_exhaustive()
    }
}

impl CameraInner {
    /// List available cameras.
    ///
    /// # Errors
    ///
    /// Returns `NotSupported` on Android until Camera2 integration is complete.
    pub const fn list() -> Result<Vec<CameraInfo>, CameraError> {
        // TODO: Implement Camera2 enumeration via JNI
        Err(CameraError::NotSupported)
    }

    /// Open a camera by ID.
    ///
    /// # Errors
    /// Returns `NotSupported` on Android until Camera2 integration is complete.
    pub async fn open(
        _camera_id: &str,
        config: CameraConfig,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
    ) -> Result<Self, CameraError> {
        // TODO: Implement Camera2 opening via JNI
        // For now, create a stub that compiles
        Ok(Self {
            device,
            queue,
            capabilities: CameraCapabilities {
                resolutions: vec![Resolution::HD],
                frame_rates: vec![30],
                iso_range: None,
                exposure_duration_range: None,
                supports_exposure_compensation: false,
                supports_manual_focus: false,
                supports_manual_white_balance: false,
                zoom_range: None,
                supports_hdr: false,
                stabilization_modes: vec![],
                has_flash: false,
                has_torch: false,
            },
            controls: CameraControls::default(),
            resolution: config.resolution,
        })
    }

    /// Get camera capabilities.
    #[must_use]
    pub const fn capabilities(&self) -> &CameraCapabilities {
        &self.capabilities
    }

    /// Apply camera controls.
    ///
    /// # Errors
    ///
    /// Returns `NotSupported` on Android.
    pub const fn apply_controls(&mut self, _controls: &CameraControls) -> Result<(), CameraError> {
        Err(CameraError::NotSupported)
    }

    /// Get current control values.
    #[must_use]
    pub const fn controls(&self) -> &CameraControls {
        &self.controls
    }

    /// Get current resolution.
    #[must_use]
    pub const fn resolution(&self) -> Resolution {
        self.resolution
    }

    /// Get frame stream.
    ///
    /// Returns an empty stream on Android until Camera2 integration is complete.
    pub fn frames(&self) -> impl futures::Stream<Item = Frame> + '_ {
        futures::stream::empty()
    }

    /// Capture a photo.
    ///
    /// # Errors
    /// Returns `NotSupported` on Android until Camera2 integration is complete.
    pub async fn capture_photo(&mut self) -> Result<Photo, CameraError> {
        Err(CameraError::NotSupported)
    }

    /// Start video recording.
    ///
    /// # Errors
    ///
    /// Returns `NotSupported` on Android.
    pub const fn start_recording(&mut self, _path: &std::path::Path) -> Result<(), CameraError> {
        Err(CameraError::NotSupported)
    }

    /// Stop video recording.
    ///
    /// # Errors
    ///
    /// Returns `NotSupported` on Android.
    pub const fn stop_recording(&mut self) -> Result<(), CameraError> {
        Err(CameraError::NotSupported)
    }

    /// Get recording duration.
    #[must_use]
    pub const fn recording_duration(&self) -> std::time::Duration {
        std::time::Duration::ZERO
    }
}
