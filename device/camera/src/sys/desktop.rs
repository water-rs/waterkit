//! Desktop camera implementation using nokhwa.
//!
//! Desktop cameras don't support professional controls (ISO, focus, etc.).
//! The camera runs in the uncompressed format closest to the requested
//! resolution and frame rate when it offers one: NV12 frames upload as
//! `YCbCr420` planes and YUYV frames as one packed `YCbCr422` texture, with no
//! CPU colour conversion. A compressed MJPEG stream is decoded on the CPU and
//! uploads as `Rgb`. Uploads go through a texture pool, and desktop frames are
//! always upright.
//!
//! Video recording runs the capture stream through `waterkit-codec` and
//! `waterkit-video-container` on a dedicated worker thread; raw recording
//! writes the uncompressed `WKRV` frame stream the mobile backends use.

mod recording;

use crate::pool::{CpuPlanes, FramePool};
use crate::{
    CameraCapabilities, CameraConfig, CameraControls, CameraError, CameraInfo, DynamicRangeProfile,
    Frame, FrameConverter, Orientation, Photo, RawPhoto, RawVideoFormat, Resolution,
    StabilizationMode, YCbCrEncoding, YCbCrMatrix, YCbCrRange,
};
use nokhwa::Camera as NokhwaCamera;
use nokhwa::pixel_format::RgbAFormat;
use nokhwa::utils::{
    CameraFormat as NokhwaCameraFormat, CameraIndex, FrameFormat as NokhwaFrameFormat,
    RequestedFormat, RequestedFormatType,
};
use recording::RecordingSession;
use std::num::NonZeroU8;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Frame formats the desktop backend runs a camera in, most preferred first:
/// the uncompressed formats upload as they are, MJPEG needs a decode.
const DELIVERED_FORMATS: [NokhwaFrameFormat; 3] = [
    NokhwaFrameFormat::NV12,
    NokhwaFrameFormat::YUYV,
    NokhwaFrameFormat::MJPEG,
];

/// How UVC webcams encode YCbCr unless a colour-matching descriptor says
/// otherwise: BT.601 (SMPTE 170M) coefficients in video range. Neither nokhwa
/// backend reports a camera's descriptor, so this is the encoding every
/// desktop YCbCr frame carries.
const WEBCAM_ENCODING: YCbCrEncoding = YCbCrEncoding {
    matrix: YCbCrMatrix::Bt601,
    range: YCbCrRange::Video,
};

/// One captured frame in the layout the camera delivered, with its capture
/// timestamp as a duration since the camera stream started.
pub(super) struct RawFrame {
    pixels: CapturedPixels,
    width: u32,
    height: u32,
    timestamp: Duration,
}

/// The pixel layouts the capture thread forwards.
pub(super) enum CapturedPixels {
    /// NV12 as the camera delivered it.
    Nv12(Vec<u8>),
    /// Packed YUYV 4:2:2 as the camera delivered it.
    Yuyv(Vec<u8>),
    /// RGBA decoded from a compressed MJPEG frame.
    Rgba(Vec<u8>),
}

impl RawFrame {
    /// Reads one nokhwa buffer, checking that its size matches its layout.
    fn capture(buffer: &nokhwa::Buffer, timestamp: Duration) -> Result<Self, CameraError> {
        let width = buffer.resolution().width();
        let height = buffer.resolution().height();
        let pixels = width as usize * height as usize;
        let (data, expected) = match buffer.source_frame_format() {
            NokhwaFrameFormat::NV12 => (
                CapturedPixels::Nv12(buffer.buffer().to_vec()),
                pixels * 3 / 2,
            ),
            NokhwaFrameFormat::YUYV => (CapturedPixels::Yuyv(buffer.buffer().to_vec()), pixels * 2),
            NokhwaFrameFormat::MJPEG => {
                let image = buffer.decode_image::<RgbAFormat>().map_err(|error| {
                    CameraError::CaptureFailed(format!("MJPEG frame decode: {error}"))
                })?;
                (CapturedPixels::Rgba(image.into_raw()), pixels * 4)
            }
            other => {
                return Err(CameraError::CaptureFailed(format!(
                    "camera delivered {other} frames, which the stream was not opened for"
                )));
            }
        };
        let len = data.bytes().len();
        if len != expected {
            return Err(CameraError::CaptureFailed(format!(
                "{width}x{height} frame holds {len} bytes, its layout needs {expected}"
            )));
        }
        Ok(Self {
            pixels: data,
            width,
            height,
            timestamp,
        })
    }

    fn upload(&self, pool: &FramePool) -> Frame {
        let planes = match &self.pixels {
            CapturedPixels::Nv12(data) => CpuPlanes::Nv12 {
                data,
                encoding: WEBCAM_ENCODING,
            },
            CapturedPixels::Yuyv(data) => CpuPlanes::Yuyv {
                data,
                encoding: WEBCAM_ENCODING,
            },
            CapturedPixels::Rgba(data) => CpuPlanes::Rgb {
                format: wgpu::TextureFormat::Rgba8Unorm,
                data,
                stride: self.width * 4,
            },
        };
        pool.upload(
            &planes,
            self.width,
            self.height,
            Orientation::Up,
            self.timestamp,
        )
    }
}

impl CapturedPixels {
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Nv12(data) | Self::Yuyv(data) | Self::Rgba(data) => data,
        }
    }
}

/// The delivered format closest to the request: nearest resolution first,
/// then nearest frame rate, then the format least work to upload.
fn choose_format(
    formats: &[NokhwaCameraFormat],
    resolution: Resolution,
    frame_rate: u32,
) -> Option<NokhwaCameraFormat> {
    formats
        .iter()
        .filter_map(|format| {
            let preference = DELIVERED_FORMATS
                .iter()
                .position(|delivered| *delivered == format.format())?;
            let resolution_distance = format.width().abs_diff(resolution.width)
                + format.height().abs_diff(resolution.height);
            let frame_rate_distance = format.frame_rate().abs_diff(frame_rate);
            Some((
                (resolution_distance, frame_rate_distance, preference),
                *format,
            ))
        })
        .min_by_key(|(key, _)| *key)
        .map(|(_, format)| format)
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
    /// Built on the first photo, so streaming alone never needs the
    /// converter's device features.
    photo_converter: OnceLock<FrameConverter>,
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

            // `frame` blocks until the camera delivers the next frame.
            let captured = camera
                .lock()
                .unwrap()
                .0
                .frame()
                .map_err(|error| CameraError::CaptureFailed(error.to_string()))
                .and_then(|buffer| {
                    RawFrame::capture(&buffer, Instant::now().duration_since(start_instant))
                });
            match captured {
                Ok(raw) => fan_out(&mut subscribers, &Arc::new(raw)),
                Err(error) => {
                    // Ending the stream closes every subscription, so the
                    // failure reaches consumers as the end of their frames.
                    tracing::error!("camera capture stopped: {error}");
                    break;
                }
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

        // Open in any delivered format, then switch to the one closest to the
        // request among everything the camera offers.
        let mut camera = NokhwaCamera::new(
            index,
            RequestedFormat::with_formats(RequestedFormatType::None, &DELIVERED_FORMATS),
        )
        .map_err(|e| CameraError::OpenFailed(e.to_string()))?;
        let offered = camera
            .compatible_camera_formats()
            .map_err(|e| CameraError::OpenFailed(e.to_string()))?;
        let format = choose_format(&offered, config.resolution, config.frame_rate.max(1))
            .ok_or_else(|| {
                CameraError::OpenFailed(format!(
                    "camera offers none of the formats the desktop backend runs ({DELIVERED_FORMATS:?}): {offered:?}"
                ))
            })?;
        camera
            .set_camera_format(format)
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
            photo_converter: OnceLock::new(),
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
        let pool = FramePool::new(Arc::clone(&self.device), Arc::clone(&self.queue));
        let receiver = self.subscribe_frames(PREVIEW_QUEUE).receiver;

        futures::stream::unfold((pool, receiver), |(pool, receiver)| async move {
            let raw = receiver.recv().await.ok()?;
            let frame = raw.upload(&pool);
            Some((frame, (pool, receiver)))
        })
    }

    /// Takes the next stream frame as the photo, converted upright on the GPU.
    pub async fn capture_photo(&self) -> Result<Photo, CameraError> {
        let raw = self
            .subscribe_frames(PREVIEW_QUEUE)
            .receiver
            .recv()
            .await
            .map_err(|_| CameraError::CaptureFailed("no frame available".into()))?;

        let pool = FramePool::new(Arc::clone(&self.device), Arc::clone(&self.queue));
        let frame = raw.upload(&pool);
        let converter = self
            .photo_converter
            .get_or_init(|| FrameConverter::new(&self.device));
        let upright = FrameConverter::create_output(&self.device, &frame);
        let size = upright.size();
        // Photos are sampled linearized, like the mobile backends' photos; the
        // converter's storage output copies into an sRGB texture of the same
        // texels.
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("CameraPhoto"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("CameraPhoto"),
            });
        converter.encode(&self.device, &mut encoder, &frame, &upright);
        encoder.copy_texture_to_texture(upright.as_image_copy(), texture.as_image_copy(), size);
        self.queue.submit([encoder.finish()]);

        Ok(Photo {
            texture,
            width: size.width,
            height: size.height,
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
            pixels: CapturedPixels::Rgba(vec![0; 4]),
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

    fn offered(width: u32, height: u32, format: NokhwaFrameFormat, fps: u32) -> NokhwaCameraFormat {
        NokhwaCameraFormat::new_from(width, height, format, fps)
    }

    /// The closest resolution wins, then the closest frame rate, and only a
    /// tie between them falls to the format that needs the least work.
    #[test]
    fn format_choice_prefers_resolution_then_frame_rate_then_uncompressed() {
        let full_hd = Resolution::FULL_HD;
        let formats = [
            offered(1920, 1080, NokhwaFrameFormat::YUYV, 5),
            offered(1920, 1080, NokhwaFrameFormat::MJPEG, 30),
            offered(1280, 720, NokhwaFrameFormat::NV12, 30),
            offered(1920, 1080, NokhwaFrameFormat::GRAY, 30),
        ];
        assert_eq!(
            choose_format(&formats, full_hd, 30),
            Some(offered(1920, 1080, NokhwaFrameFormat::MJPEG, 30))
        );
        assert_eq!(
            choose_format(&formats, full_hd, 5),
            Some(offered(1920, 1080, NokhwaFrameFormat::YUYV, 5))
        );
        let tied = [
            offered(1280, 720, NokhwaFrameFormat::MJPEG, 30),
            offered(1280, 720, NokhwaFrameFormat::YUYV, 30),
            offered(1280, 720, NokhwaFrameFormat::NV12, 30),
        ];
        assert_eq!(
            choose_format(&tied, Resolution::HD, 30),
            Some(offered(1280, 720, NokhwaFrameFormat::NV12, 30))
        );
        assert_eq!(
            choose_format(
                &[offered(640, 480, NokhwaFrameFormat::RAWRGB, 30)],
                Resolution::HD,
                30
            ),
            None
        );
    }
}
