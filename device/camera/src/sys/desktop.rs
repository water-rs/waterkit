//! Desktop camera implementation using nokhwa.
//!
//! Desktop cameras don't support professional controls (ISO, focus, etc.).
//! Frames are uploaded to GPU textures via CPU copy. Video recording runs the
//! capture stream through `waterkit-codec` and `waterkit-video-container` on a
//! dedicated worker thread; raw recording writes the uncompressed `WKRV`
//! frame stream the mobile backends use.

mod recording;

use crate::{
    CameraCapabilities, CameraConfig, CameraControls, CameraError, CameraInfo, DynamicRangeProfile,
    Frame, Photo, PixelFormat, RawPhoto, RawVideoFormat, Resolution, StabilizationMode,
};
use nokhwa::Camera as NokhwaCamera;
use nokhwa::pixel_format::RgbAFormat;
use nokhwa::utils::{
    CameraFormat as NokhwaCameraFormat, CameraIndex, FrameFormat as NokhwaFrameFormat,
    RequestedFormat, RequestedFormatType, Resolution as NokhwaResolution,
};
use recording::RecordingSession;
use std::num::NonZeroU8;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Internal frame data from nokhwa: decoded RGBA pixels and the capture
/// timestamp as a duration since the camera stream started.
pub(super) struct RawFrame {
    data: Vec<u8>,
    width: u32,
    height: u32,
    timestamp: Duration,
}

/// One live frame subscription, owned by the capture thread.
struct Subscriber {
    sender: async_channel::Sender<Arc<RawFrame>>,
    /// Frames displaced by `force_send` before the receiver could read them.
    dropped: Arc<AtomicU64>,
}

/// A capture-stream receiver plus the count of frames it missed because a
/// newer frame displaced the pending one before it was read.
pub(super) struct FrameSubscription {
    pub receiver: async_channel::Receiver<Arc<RawFrame>>,
    pub dropped: Arc<AtomicU64>,
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
    /// Registrations travel to the capture thread over this unbounded
    /// channel; the subscriber vector itself is owned by that thread alone.
    subscriber_tx: async_channel::Sender<Subscriber>,
    streaming: Arc<AtomicBool>,
    frame_rate: u32,
    recording: Option<RecordingSession>,
}

/// Preview subscribers only ever need the newest frame.
const PREVIEW_QUEUE: usize = 1;
/// A recording may lag briefly on scheduler jitter before frames must drop.
const RECORDING_QUEUE: usize = 4;

fn parse_camera_index(camera_id: &str) -> CameraIndex {
    camera_id.parse::<u32>().map_or_else(
        |_| CameraIndex::String(camera_id.to_string()),
        CameraIndex::Index,
    )
}

fn build_desktop_capabilities(
    detected_resolution: Resolution,
    config: &CameraConfig,
) -> Result<CameraCapabilities, CameraError> {
    let mut resolutions = Vec::with_capacity(4);
    resolutions.push(detected_resolution);
    if !resolutions.contains(&config.resolution) {
        resolutions.push(config.resolution);
    }
    if !resolutions.contains(&Resolution::HD) {
        resolutions.push(Resolution::HD);
    }
    if !resolutions.contains(&Resolution::FULL_HD) {
        resolutions.push(Resolution::FULL_HD);
    }

    let mut frame_rates = vec![config.frame_rate.max(1)];
    if !frame_rates.contains(&30) {
        frame_rates.push(30);
    }

    let capabilities = CameraCapabilities {
        resolutions,
        frame_rates,
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
        supports_raw_video: true,
        raw_video_formats: vec![RawVideoFormat::Rgba8Frames],
    };
    capabilities.validate()?;
    Ok(capabilities)
}

/// Deliver `frame` to every live subscriber. A lagging subscriber's pending
/// frame is displaced by the newer one rather than applying backpressure to
/// the capture thread; each displaced frame is counted on the subscription.
fn fan_out(subscribers: &mut Vec<Subscriber>, frame: &Arc<RawFrame>) {
    subscribers.retain(|sub| !sub.sender.is_closed());
    for sub in subscribers.iter() {
        if let Ok(Some(_evicted)) = sub.sender.force_send(Arc::clone(frame)) {
            sub.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn spawn_capture_thread(
    camera: Arc<Mutex<SendableCamera>>,
    registration_rx: async_channel::Receiver<Subscriber>,
    streaming: Arc<AtomicBool>,
    start_instant: Instant,
) {
    std::thread::spawn(move || {
        let mut subscribers: Vec<Subscriber> = Vec::new();
        while streaming.load(Ordering::SeqCst) {
            // Pick up subscriptions registered since the last frame.
            while let Ok(subscriber) = registration_rx.try_recv() {
                subscribers.push(subscriber);
            }

            let frame = {
                let mut guard = camera.lock().unwrap();
                guard.0.frame().ok()
            };

            if let Some(frame) = frame {
                let decoded = frame.decode_image::<RgbAFormat>();
                if let Ok(img) = decoded {
                    let raw = Arc::new(RawFrame {
                        data: img.into_raw(),
                        width: frame.resolution().width(),
                        height: frame.resolution().height(),
                        timestamp: Instant::now().duration_since(start_instant),
                    });
                    fan_out(&mut subscribers, &raw);
                }
            } else {
                std::thread::sleep(Duration::from_millis(2));
            }
        }

        let mut guard = camera.lock().unwrap();
        let _ = guard.0.stop_stream();
    });
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

    #[allow(
        clippy::unused_async,
        reason = "Camera opening is async on mobile backends; the desktop backend keeps the same platform abstraction surface."
    )]
    pub async fn open(
        camera_id: &str,
        config: CameraConfig,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
    ) -> Result<Self, CameraError> {
        let index = parse_camera_index(camera_id);

        let requested_format = NokhwaCameraFormat::new(
            NokhwaResolution::new(config.resolution.width, config.resolution.height),
            NokhwaFrameFormat::RAWRGB,
            config.frame_rate.max(1),
        );
        let requested =
            RequestedFormat::new::<RgbAFormat>(RequestedFormatType::Closest(requested_format));

        let mut camera = NokhwaCamera::new(index, requested)
            .map_err(|e| CameraError::OpenFailed(e.to_string()))?;

        let resolution = camera.resolution();
        let res = Resolution {
            width: resolution.width(),
            height: resolution.height(),
        };

        let capabilities = build_desktop_capabilities(res, &config)?;

        // Start streaming immediately (RAII)
        camera
            .open_stream()
            .map_err(|e| CameraError::StartFailed(e.to_string()))?;

        let streaming = Arc::new(AtomicBool::new(true));
        let start_instant = Instant::now();

        // Wrap camera in SendableCamera for thread safety
        let camera = Arc::new(Mutex::new(SendableCamera(camera)));
        let (subscriber_tx, subscriber_rx) = async_channel::unbounded();
        spawn_capture_thread(camera, subscriber_rx, Arc::clone(&streaming), start_instant);

        Ok(Self {
            device,
            queue,
            resolution: res,
            capabilities,
            controls: CameraControls::default(),
            subscriber_tx,
            streaming,
            frame_rate: config.frame_rate.max(1),
            recording: None,
        })
    }

    /// Subscribe a new receiver to the capture stream with `capacity`
    /// queued frames. Every subscriber sees every frame until it falls
    /// `capacity` behind; further frames then displace its oldest pending
    /// frame and are counted on the subscription's `dropped` tally.
    fn subscribe_frames(&self, capacity: usize) -> FrameSubscription {
        let (sender, receiver) = async_channel::bounded(capacity);
        let dropped = Arc::new(AtomicU64::new(0));
        // Unbounded, so registration never blocks. When the capture thread
        // has already exited the receiver simply yields no frames.
        let _ = self.subscriber_tx.try_send(Subscriber {
            sender,
            dropped: Arc::clone(&dropped),
        });
        FrameSubscription { receiver, dropped }
    }

    pub const fn capabilities(&self) -> &CameraCapabilities {
        &self.capabilities
    }

    pub fn apply_controls(&mut self, controls: &CameraControls) -> Result<(), CameraError> {
        // Desktop cameras don't support professional controls
        if controls.exposure.is_some() {
            return Err(CameraError::ControlUnsupported("exposure".into()));
        }
        if controls.focus.is_some() {
            return Err(CameraError::ControlUnsupported("focus".into()));
        }
        if controls.white_balance.is_some() {
            return Err(CameraError::ControlUnsupported("white_balance".into()));
        }
        if controls.zoom.is_some() {
            return Err(CameraError::ControlUnsupported("zoom".into()));
        }
        if controls.flash.is_some() {
            return Err(CameraError::ControlUnsupported("flash".into()));
        }
        if controls.dynamic_range.is_some() {
            return Err(CameraError::ControlUnsupported("dynamic_range".into()));
        }
        if controls.stabilization.is_some() {
            return Err(CameraError::ControlUnsupported("stabilization".into()));
        }
        self.controls = controls.clone();
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
        let receiver = self.subscribe_frames(PREVIEW_QUEUE).receiver;

        futures::stream::unfold(
            (device, queue, receiver),
            move |(device, queue, receiver)| async move {
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

                let frame = Frame {
                    texture,
                    width: raw.width,
                    height: raw.height,
                    format: PixelFormat::Rgba8,
                    timestamp: raw.timestamp,
                };

                Some((frame, (device, queue, receiver)))
            },
        )
    }

    pub async fn capture_photo(&self) -> Result<Photo, CameraError> {
        // Wait for next frame from the stream
        let raw = self
            .subscribe_frames(PREVIEW_QUEUE)
            .receiver
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

    #[allow(
        clippy::unused_self,
        clippy::unused_async,
        reason = "RAW capture is async on supported platform backends; desktop reports the unsupported capability through the same API."
    )]
    pub async fn capture_raw_photo(&self) -> Result<RawPhoto, CameraError> {
        Err(CameraError::ControlUnsupported(
            "raw photo not supported on desktop".into(),
        ))
    }

    pub fn start_recording(&mut self, path: &Path) -> Result<(), CameraError> {
        if self.recording.is_some() {
            return Err(CameraError::AlreadyInUse);
        }
        let session = RecordingSession::compressed(
            path,
            self.subscribe_frames(RECORDING_QUEUE),
            self.resolution.width,
            self.resolution.height,
            self.frame_rate,
        )?;
        self.recording = Some(session);
        Ok(())
    }

    pub fn stop_recording(&mut self) -> Result<(), CameraError> {
        self.recording.take().map_or(Ok(()), RecordingSession::stop)
    }

    pub fn recording_duration(&self) -> Duration {
        self.recording
            .as_ref()
            .map_or(Duration::ZERO, RecordingSession::duration)
    }

    pub fn start_raw_recording(&mut self, path: &Path) -> Result<(), CameraError> {
        if self.recording.is_some() {
            return Err(CameraError::AlreadyInUse);
        }
        let session = RecordingSession::raw(
            path,
            self.subscribe_frames(RECORDING_QUEUE),
            self.resolution.width,
            self.resolution.height,
            self.frame_rate,
        )?;
        self.recording = Some(session);
        Ok(())
    }

    pub fn stop_raw_recording(&mut self) -> Result<(), CameraError> {
        self.recording.take().map_or(Ok(()), RecordingSession::stop)
    }

    pub fn raw_recording_duration(&self) -> Duration {
        self.recording
            .as_ref()
            .map_or(Duration::ZERO, RecordingSession::duration)
    }
}

impl Drop for CameraInner {
    fn drop(&mut self) {
        // Signal the capture thread to stop
        self.streaming.store(false, Ordering::SeqCst);
        if let Some(session) = self.recording.take() {
            let _ = session.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lagging subscriber loses its pending frame to the newest one, and
    /// the loss is counted on the subscription rather than hidden.
    #[test]
    fn fan_out_counts_frames_displaced_for_a_lagging_subscriber() {
        let (sender, receiver) = async_channel::bounded(1);
        let dropped = Arc::new(AtomicU64::new(0));
        let mut subscribers = vec![Subscriber {
            sender,
            dropped: Arc::clone(&dropped),
        }];
        let frame = Arc::new(RawFrame {
            data: vec![0],
            width: 1,
            height: 1,
            timestamp: Duration::ZERO,
        });
        fan_out(&mut subscribers, &frame);
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
        // The channel still holds the first frame, so the second displaces it.
        fan_out(&mut subscribers, &frame);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        receiver.try_recv().unwrap();
        fan_out(&mut subscribers, &frame);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        // A closed receiver is pruned from the list.
        drop(receiver);
        fan_out(&mut subscribers, &frame);
        assert!(subscribers.is_empty());
    }
}
