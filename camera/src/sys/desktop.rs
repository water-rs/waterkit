//! Desktop camera implementation using nokhwa.

use crate::{CameraError, CameraFrame, CameraInfo, FrameFormat, Resolution};
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{CameraIndex, RequestedFormat, RequestedFormatType};
use nokhwa::Camera as NokhwaCamera;
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub struct CameraInner {
    camera: Arc<Mutex<Option<NokhwaCamera>>>,
    camera_id: String,
    resolution: Resolution,
}

impl CameraInner {
    pub fn list() -> Result<Vec<CameraInfo>, CameraError> {
        let devices = nokhwa::query(nokhwa::utils::ApiBackend::Auto)
            .map_err(|e| CameraError::EnumerationFailed(e.to_string()))?;

        Ok(devices
            .into_iter()
            .map(|d| CameraInfo {
                id: d.index().to_string(),
                name: d.human_name(),
                description: Some(d.description().to_string()),
                is_front_facing: false, // Desktop cameras don't typically have this info
            })
            .collect())
    }

    pub fn open(camera_id: &str) -> Result<Self, CameraError> {
        let index = camera_id
            .parse::<u32>()
            .map(CameraIndex::Index)
            .unwrap_or_else(|_| CameraIndex::String(camera_id.to_string()));

        let requested = RequestedFormat::<RgbFormat>::new(RequestedFormatType::HighestResolution(
            nokhwa::utils::Resolution::new(1280, 720),
        ));

        let camera = NokhwaCamera::new(index, requested)
            .map_err(|e| CameraError::OpenFailed(e.to_string()))?;

        let resolution = camera.resolution();

        Ok(Self {
            camera: Arc::new(Mutex::new(Some(camera))),
            camera_id: camera_id.to_string(),
            resolution: Resolution {
                width: resolution.width(),
                height: resolution.height(),
            },
        })
    }

    pub fn start(&mut self) -> Result<(), CameraError> {
        let mut guard = self.camera.lock().unwrap();
        if let Some(camera) = guard.as_mut() {
            camera
                .open_stream()
                .map_err(|e| CameraError::StartFailed(e.to_string()))?;
        }
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), CameraError> {
        let mut guard = self.camera.lock().unwrap();
        if let Some(camera) = guard.as_mut() {
            camera
                .stop_stream()
                .map_err(|e| CameraError::CaptureFailed(e.to_string()))?;
        }
        Ok(())
    }

    pub fn get_frame(&mut self) -> Result<CameraFrame, CameraError> {
        let mut guard = self.camera.lock().unwrap();
        let camera = guard
            .as_mut()
            .ok_or_else(|| CameraError::CaptureFailed("camera not opened".into()))?;

        let frame = camera
            .frame()
            .map_err(|e| CameraError::CaptureFailed(e.to_string()))?;

        let decoded = frame
            .decode_image::<RgbFormat>()
            .map_err(|e| CameraError::CaptureFailed(e.to_string()))?;

        Ok(CameraFrame::new(
            decoded.into_raw(),
            self.resolution.width,
            self.resolution.height,
            FrameFormat::Rgb,
            None,
        ))
    }

    pub fn set_resolution(&mut self, resolution: Resolution) -> Result<(), CameraError> {
        let mut guard = self.camera.lock().unwrap();
        if let Some(camera) = guard.as_mut() {
            camera
                .set_resolution(nokhwa::utils::Resolution::new(
                    resolution.width,
                    resolution.height,
                ))
                .map_err(|e| CameraError::OpenFailed(e.to_string()))?;
            self.resolution = resolution;
        }
        Ok(())
    }

    pub fn resolution(&self) -> Resolution {
        self.resolution
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
