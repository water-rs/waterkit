//! Platform-specific camera implementations.

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;

#[cfg(target_os = "android")]
mod android;

#[cfg(any(target_os = "windows", target_os = "linux"))]
mod desktop;

// Apple platforms
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub(crate) use apple::CameraInner;

// Android
#[cfg(target_os = "android")]
pub(crate) use android::CameraInner;

// Desktop (Windows, Linux) - use nokhwa
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub(crate) use desktop::CameraInner;

// Fallback for unsupported platforms
#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
mod fallback {
    use super::*;

    #[derive(Debug)]
    pub struct CameraInner;

    impl CameraInner {
        pub fn list() -> Result<Vec<CameraInfo>, CameraError> {
            Err(CameraError::NotSupported)
        }

        pub fn open(_camera_id: &str) -> Result<Self, CameraError> {
            Err(CameraError::NotSupported)
        }

        pub fn start(&mut self) -> Result<(), CameraError> {
            Err(CameraError::NotSupported)
        }

        pub fn stop(&mut self) -> Result<(), CameraError> {
            Err(CameraError::NotSupported)
        }

        pub fn get_frame(&mut self) -> Result<CameraFrame, CameraError> {
            Err(CameraError::NotSupported)
        }

        pub fn set_resolution(&mut self, _resolution: Resolution) -> Result<(), CameraError> {
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
    }
}

#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_os = "windows",
    target_os = "linux"
)))]
pub(crate) use fallback::CameraInner;

// Export NativeHandle for platform-specific zero-copy access
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub type NativeHandle = apple::IOSurfaceHandle;

#[cfg(not(any(target_os = "ios", target_os = "macos")))]
#[derive(Debug, Clone, Copy)]
pub struct NativeHandle;
