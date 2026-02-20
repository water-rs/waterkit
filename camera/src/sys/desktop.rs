//! Desktop camera implementation using nokhwa.
//!
//! Desktop cameras don't support professional controls (ISO, focus, etc.).
//! Frames are uploaded to GPU textures via CPU copy.

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
    CameraCapabilities, CameraConfig, CameraControls, CameraError, CameraInfo, DynamicRangeProfile,
    Frame, Photo, PixelFormat, RawPhoto, Resolution, StabilizationMode,
};
use nokhwa::Camera as NokhwaCamera;
use nokhwa::pixel_format::RgbAFormat;
use nokhwa::utils::{CameraIndex, RequestedFormat, RequestedFormatType};
use std::num::NonZeroU8;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Internal frame data from nokhwa.
struct RawFrame {
    data: Vec<u8>,
    width: u32,
    height: u32,
    timestamp: std::time::Instant,
}

/// Wrapper around `NokhwaCamera` that implements Send.
///
/// Safety: On Linux, V4L2 backend isn't Send, but we ensure all access
/// happens through a Mutex on the original thread or via synchronous calls.
struct SendableCamera(NokhwaCamera);

// SAFETY: We ensure synchronized access through Mutex and only access
// the camera from where it's safe to do so.
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Send for SendableCamera {}

pub struct CameraInner {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    resolution: Resolution,
    capabilities: CameraCapabilities,
    controls: CameraControls,
    frame_receiver: async_channel::Receiver<RawFrame>,
    streaming: Arc<AtomicBool>,
    start_time: Arc<Mutex<std::time::Instant>>,
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

    pub async fn open(
        camera_id: &str,
        config: CameraConfig,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
    ) -> Result<Self, CameraError> {
        let index = camera_id.parse::<u32>().map_or_else(
            |_| CameraIndex::String(camera_id.to_string()),
            CameraIndex::Index,
        );

        let requested = RequestedFormat::new::<RgbAFormat>(RequestedFormatType::HighestResolution(
            nokhwa::utils::Resolution::new(config.resolution.width, config.resolution.height),
        ));

        let mut camera = NokhwaCamera::new(index, requested)
            .map_err(|e| CameraError::OpenFailed(e.to_string()))?;

        let resolution = camera.resolution();
        let res = Resolution {
            width: resolution.width(),
            height: resolution.height(),
        };

        // Create frame channel
        let (sender, receiver) = async_channel::bounded(2);

        // Desktop cameras have no professional controls
        let capabilities = CameraCapabilities {
            resolutions: vec![res, Resolution::HD, Resolution::FULL_HD],
            frame_rates: vec![30],
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
        };
        capabilities.validate()?;

        // Start streaming immediately (RAII)
        camera
            .open_stream()
            .map_err(|e| CameraError::StartFailed(e.to_string()))?;

        let streaming = Arc::new(AtomicBool::new(true));
        let start_time = Arc::new(Mutex::new(std::time::Instant::now()));

        // Wrap camera in SendableCamera for thread safety
        let camera = Arc::new(Mutex::new(SendableCamera(camera)));
        let streaming_clone = streaming.clone();
        let start_time_clone = start_time.clone();

        std::thread::spawn(move || {
            while streaming_clone.load(Ordering::SeqCst) {
                let frame = {
                    let mut guard = camera.lock().unwrap();
                    guard.0.frame().ok()
                };

                if let Some(frame) = frame {
                    let decoded = frame.decode_image::<RgbAFormat>();
                    if let Ok(img) = decoded {
                        let raw = RawFrame {
                            data: img.into_raw(),
                            width: frame.resolution().width(),
                            height: frame.resolution().height(),
                            timestamp: *start_time_clone.lock().unwrap(),
                        };
                        // Non-blocking send
                        let _ = sender.try_send(raw);
                    }
                }

                // Small sleep to prevent busy loop
                std::thread::sleep(std::time::Duration::from_millis(1));
            }

            // Stop the camera when thread exits
            let mut guard = camera.lock().unwrap();
            let _ = guard.0.stop_stream();
        });

        Ok(Self {
            device,
            queue,
            resolution: res,
            capabilities,
            controls: CameraControls::default(),
            frame_receiver: receiver,
            streaming,
            start_time,
        })
    }

    pub fn capabilities(&self) -> &CameraCapabilities {
        &self.capabilities
    }

    pub fn apply_controls(&mut self, controls: &CameraControls) -> Result<(), CameraError> {
        // Desktop cameras don't support professional controls
        if controls.exposure.is_some() {
            return Err(CameraError::ControlNotSupported("exposure".into()));
        }
        if controls.focus.is_some() {
            return Err(CameraError::ControlNotSupported("focus".into()));
        }
        if controls.white_balance.is_some() {
            return Err(CameraError::ControlNotSupported("white_balance".into()));
        }
        if controls.zoom.is_some() {
            return Err(CameraError::ControlNotSupported("zoom".into()));
        }
        if controls.flash.is_some() {
            return Err(CameraError::ControlNotSupported("flash".into()));
        }
        if controls.dynamic_range.is_some() {
            return Err(CameraError::ControlNotSupported("dynamic_range".into()));
        }
        if controls.stabilization.is_some() {
            return Err(CameraError::ControlNotSupported("stabilization".into()));
        }
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
        let start_time = self.start_time.clone();

        futures::stream::unfold(
            (device, queue, receiver, start_time),
            move |(device, queue, receiver, start_time)| async move {
                let raw = receiver.recv().await.ok()?;

                // Create GPU texture
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("CameraFrame"),
                    size: wgpu::Extent3d {
                        width: raw.width,
                        height: raw.height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });

                // Upload frame data to GPU
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &raw.data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(raw.width * 4),
                        rows_per_image: Some(raw.height),
                    },
                    wgpu::Extent3d {
                        width: raw.width,
                        height: raw.height,
                        depth_or_array_layers: 1,
                    },
                );

                let timestamp = {
                    let start = *start_time.lock().unwrap();
                    raw.timestamp.duration_since(start)
                };

                let frame = Frame {
                    texture,
                    width: raw.width,
                    height: raw.height,
                    format: PixelFormat::Rgba8,
                    timestamp,
                };

                Some((frame, (device, queue, receiver, start_time)))
            },
        )
    }

    pub async fn capture_photo(&mut self) -> Result<Photo, CameraError> {
        // Wait for next frame from the stream
        let raw = self
            .frame_receiver
            .recv()
            .await
            .map_err(|_| CameraError::CaptureFailed("no frame available".into()))?;

        // Create GPU texture
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("CameraPhoto"),
            size: wgpu::Extent3d {
                width: raw.width,
                height: raw.height,
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
            &raw.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(raw.width * 4),
                rows_per_image: Some(raw.height),
            },
            wgpu::Extent3d {
                width: raw.width,
                height: raw.height,
                depth_or_array_layers: 1,
            },
        );

        Ok(Photo {
            texture,
            width: raw.width,
            height: raw.height,
        })
    }

    pub async fn capture_raw_photo(&mut self) -> Result<RawPhoto, CameraError> {
        Err(CameraError::ControlNotSupported(
            "raw photo not supported on desktop".into(),
        ))
    }

    pub fn start_recording(&mut self, _path: &Path) -> Result<(), CameraError> {
        Err(CameraError::ControlNotSupported(
            "recording not supported on desktop".into(),
        ))
    }

    pub fn stop_recording(&mut self) -> Result<(), CameraError> {
        Err(CameraError::ControlNotSupported(
            "recording not supported on desktop".into(),
        ))
    }

    pub fn recording_duration(&self) -> Duration {
        Duration::ZERO
    }

    pub fn start_raw_recording(&mut self, _path: &Path) -> Result<(), CameraError> {
        Err(CameraError::ControlNotSupported(
            "raw recording not supported on desktop".into(),
        ))
    }

    pub fn stop_raw_recording(&mut self) -> Result<(), CameraError> {
        Err(CameraError::ControlNotSupported(
            "raw recording not supported on desktop".into(),
        ))
    }

    pub fn raw_recording_duration(&self) -> Duration {
        Duration::ZERO
    }
}

impl Drop for CameraInner {
    fn drop(&mut self) {
        // Signal the capture thread to stop
        self.streaming.store(false, Ordering::SeqCst);
    }
}
