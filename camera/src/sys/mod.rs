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
    use crate::{CameraError, CameraFrame, CameraInfo, Resolution};

    #[derive(Debug)]
    pub struct CameraInner;

    impl CameraInner {
        pub fn list() -> Result<Vec<CameraInfo>, CameraError> {
            Err(CameraError::NotSupported)
        }

        pub fn open(_camera_id: &str) -> Result<Self, CameraError> {
            Err(CameraError::NotSupported)
        }

        pub fn start(&self) -> Result<(), CameraError> {
            Err(CameraError::NotSupported)
        }

        pub fn stop(&self) -> Result<(), CameraError> {
            Err(CameraError::NotSupported)
        }

        pub fn get_frame(&self) -> Result<CameraFrame, CameraError> {
            Err(CameraError::NotSupported)
        }

        pub fn set_resolution(&self, _resolution: Resolution) -> Result<(), CameraError> {
            Err(CameraError::NotSupported)
        }

        pub fn resolution(&self) -> Resolution {
            Resolution::HD
        }

        pub fn dropped_frame_count(&self) -> u64 {
            0
        }

        pub fn set_hdr(&self, _enabled: bool) -> Result<(), CameraError> {
            Err(CameraError::NotSupported)
        }

        pub fn hdr_enabled(&self) -> bool {
            false
        }

        pub fn take_photo(&self) -> Result<CameraFrame, CameraError> {
            Err(CameraError::NotSupported)
        }

        pub fn start_recording(&self, _path: &str) -> Result<(), CameraError> {
            Err(CameraError::NotSupported)
        }

        pub fn stop_recording(&self) -> Result<(), CameraError> {
            Err(CameraError::NotSupported)
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

// Export NativeHandle for platform-specific zero-copy access
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub type NativeHandle = apple::IOSurfaceHandle;

#[cfg(not(any(target_os = "ios", target_os = "macos")))]
/// Opaque handle for platform-specific zero-copy frame access.
#[derive(Debug, Clone, Copy)]
pub struct NativeHandle;
