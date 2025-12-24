//! Apple platform (iOS/macOS) camera implementation using AVCaptureSession.
//!
//! Uses Metal texture interop for zero-copy frame rendering with wgpu.

use crate::{CameraError, CameraFrame, CameraInfo, FrameFormat, Resolution};
use std::sync::{Arc, Mutex};

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
        fn camera_device_count() -> i32;
        fn camera_device_id(index: i32) -> String;
        fn camera_device_name(index: i32) -> String;
        fn camera_device_description(index: i32) -> String;
        fn camera_device_is_front(index: i32) -> bool;

        fn camera_open(device_id: String) -> CameraResultFFI;
        fn camera_start() -> CameraResultFFI;
        fn camera_stop() -> CameraResultFFI;

        fn camera_has_frame() -> bool;
        fn camera_frame_width() -> u32;
        fn camera_frame_height() -> u32;
        fn camera_frame_format() -> u8;

        fn camera_get_iosurface() -> u64;
        fn camera_retain_iosurface(handle: u64);
        fn camera_release_iosurface(handle: u64);
        fn camera_consume_frame();

        fn camera_set_resolution(width: u32, height: u32) -> CameraResultFFI;
        fn camera_get_resolution_width() -> u32;
        fn camera_get_resolution_height() -> u32;
        fn camera_get_dropped_frame_count() -> u64;

        fn camera_set_hdr(enabled: bool) -> CameraResultFFI;
        fn camera_get_hdr() -> bool;

        fn camera_take_photo() -> CameraResultFFI;
        fn camera_get_photo_len() -> i32;
        fn camera_start_recording(path: String) -> CameraResultFFI;
        fn camera_stop_recording() -> CameraResultFFI;
    }
}

// External C function to bypass swift-bridge limitations for raw pointer
unsafe extern "C" {
    fn camera_copy_frame_data(buffer: *mut u8, size: usize);
    fn camera_copy_photo_data(buffer: *mut u8, size: u64);
}

fn convert_result(result: ffi::CameraResultFFI, context: &str) -> Result<(), CameraError> {
    match result {
        ffi::CameraResultFFI::Success => Ok(()),
        ffi::CameraResultFFI::NotSupported => Err(CameraError::NotSupported),
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

fn convert_format(format: u8) -> FrameFormat {
    match format {
        0 => FrameFormat::Rgb,
        1 => FrameFormat::Rgba,
        2 => FrameFormat::Bgra,
        3 => FrameFormat::Nv12,
        4 => FrameFormat::Yuy2,
        _ => FrameFormat::Bgra,
    }
}

/// Raw IOSurface handle for zero-copy Metal texture import.
#[derive(Debug)]
pub struct IOSurfaceHandle(pub u64);

impl Clone for IOSurfaceHandle {
    fn clone(&self) -> Self {
        if self.0 != 0 {
            ffi::camera_retain_iosurface(self.0);
        }
        Self(self.0)
    }
}

impl Drop for IOSurfaceHandle {
    fn drop(&mut self) {
        if self.0 != 0 {
            ffi::camera_release_iosurface(self.0);
        }
    }
}

impl IOSurfaceHandle {
    /// Check if this is a valid handle.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.0 != 0
    }

    /// Get the raw pointer value.
    #[must_use]
    pub const fn as_ptr(&self) -> *mut std::ffi::c_void {
        self.0 as *mut std::ffi::c_void
    }
}

/// Camera frame with optional `IOSurface` for zero-copy GPU access.
#[derive(Debug, Clone)]
pub struct NativeFrame {
    /// Width in pixels
    pub width: u32,
    /// Height in pixels
    pub height: u32,
    /// Pixel format
    pub format: FrameFormat,
    /// `IOSurface` handle for zero-copy Metal texture creation
    pub iosurface: IOSurfaceHandle,
}

/// Internal camera backend for Apple platforms.
#[derive(Debug)]
pub struct CameraInner {
    resolution: Arc<Mutex<Resolution>>,
}

impl CameraInner {
    /// List available camera devices.
    ///
    /// # Errors
    /// Returns a `CameraError` if enumeration fails.
    #[allow(clippy::unnecessary_wraps)]
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
    ///
    /// # Errors
    /// Returns a `CameraError` if the camera cannot be opened.
    pub fn open(camera_id: &str) -> Result<Self, CameraError> {
        convert_result(ffi::camera_open(camera_id.to_string()), camera_id)?;
        let w = ffi::camera_get_resolution_width();
        let h = ffi::camera_get_resolution_height();
        Ok(Self {
            resolution: Arc::new(Mutex::new(Resolution {
                width: w,
                height: h,
            })),
        })
    }

    /// Start the camera session.
    ///
    /// # Errors
    /// Returns a `CameraError` if the camera cannot be started.
    #[allow(clippy::unused_self)]
    pub fn start(&self) -> Result<(), CameraError> {
        convert_result(ffi::camera_start(), "start")
    }

    /// Stop the camera session.
    ///
    /// # Errors
    /// Returns a `CameraError` if the camera cannot be stopped.
    #[allow(clippy::unused_self)]
    pub fn stop(&self) -> Result<(), CameraError> {
        convert_result(ffi::camera_stop(), "stop")
    }

    /// Get the native frame with IOSurface handle for zero-copy GPU access.
    ///
    /// # Errors
    /// Returns a `CameraError` if no frame is available.
    #[allow(clippy::unused_self)]
    pub fn get_native_frame(&self) -> Result<NativeFrame, CameraError> {
        if !ffi::camera_has_frame() {
            return Err(CameraError::CaptureFailed("no frame available".into()));
        }

        let width = ffi::camera_frame_width();
        let height = ffi::camera_frame_height();
        let format = ffi::camera_frame_format();
        let iosurface = ffi::camera_get_iosurface();

        Ok(NativeFrame {
            width,
            height,
            format: convert_format(format),
            iosurface: IOSurfaceHandle(iosurface),
        })
    }

    /// Consume the current frame (call after processing).
    #[allow(clippy::unused_self)]
    pub fn consume_frame(&self) {
        ffi::camera_consume_frame();
    }

    /// Get a camera frame.
    ///
    /// # Errors
    /// Returns a `CameraError` if frame capture fails.
    pub fn get_frame(&self) -> Result<CameraFrame, CameraError> {
        // Get native frame info and zero-copy handle
        let native = self.get_native_frame()?;

        // Also copy data to CPU buffer for compatibility
        // This is necessary because wgpu texture creation from IOSurface
        // is not yet fully implemented or might be optional.
        let bytes_per_pixel = native.format.bytes_per_pixel();
        let size = (native.width * native.height) as usize * bytes_per_pixel;
        let mut data = vec![0u8; size];
        
        unsafe {
            camera_copy_frame_data(data.as_mut_ptr(), size);
        }
        
        self.consume_frame();
        
        Ok(CameraFrame::new(
            data,
            native.width,
            native.height,
            native.format,
            Some(native.iosurface),
        ))
    }

    /// Set camera resolution.
    ///
    /// # Errors
    /// Returns a `CameraError` if the resolution cannot be set.
    pub fn set_resolution(&self, resolution: Resolution) -> Result<(), CameraError> {
        convert_result(
            ffi::camera_set_resolution(resolution.width, resolution.height),
            "set_resolution",
        )?;
        *self.resolution.lock().unwrap() = resolution;
        Ok(())
    }

    /// Get current resolution.
    #[must_use]
    pub fn resolution(&self) -> Resolution {
        *self.resolution.lock().unwrap()
    }
    
    /// Get dropped frame count.
    #[must_use]
    #[allow(clippy::unused_self)]
    pub fn dropped_frame_count(&self) -> u64 {
        ffi::camera_get_dropped_frame_count()
    }

    /// Set HDR mode.
    ///
    /// # Errors
    /// Returns a `CameraError` if HDR cannot be set.
    #[allow(clippy::unused_self)]
    pub fn set_hdr(&self, enabled: bool) -> Result<(), CameraError> {
        convert_result(ffi::camera_set_hdr(enabled), "set_hdr")
    }

    /// Check if HDR is enabled.
    #[must_use]
    #[allow(clippy::unused_self)]
    pub fn hdr_enabled(&self) -> bool {
        ffi::camera_get_hdr()
    }

    /// Take a photo.
    ///
    /// # Errors
    /// Returns a `CameraError` if the photo cannot be taken.
    pub fn take_photo(&self) -> Result<CameraFrame, CameraError> {
        convert_result(ffi::camera_take_photo(), "take_photo")?;
        
        let len = ffi::camera_get_photo_len();
        if len <= 0 {
             return Err(CameraError::CaptureFailed("Empty photo data".into()));
        }
        
        #[allow(clippy::cast_sign_loss)]
        let mut data = vec![0u8; len as usize];
        unsafe {
            #[allow(clippy::cast_sign_loss)]
            camera_copy_photo_data(data.as_mut_ptr(), len as u64);
        }
        
        // Return with current resolution (though JPEG might differ)
        let res = self.resolution();
        
        Ok(CameraFrame::new(
            data,
            res.width, 
            res.height,
            FrameFormat::Jpeg,
            None
        ))
    }

    /// Start recording video.
    ///
    /// # Errors
    /// Returns a `CameraError` if recording cannot be started.
    #[allow(clippy::unused_self)]
    pub fn start_recording(&self, path: &str) -> Result<(), CameraError> {
        convert_result(ffi::camera_start_recording(path.to_string()), "start_recording")
    }

    /// Stop recording video.
    ///
    /// # Errors
    /// Returns a `CameraError` if recording cannot be stopped.
    #[allow(clippy::unused_self)]
    pub fn stop_recording(&self) -> Result<(), CameraError> {
        convert_result(ffi::camera_stop_recording(), "stop_recording")
    }
}
