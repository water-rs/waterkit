//! Camera case: streams frames from every camera on the device and reports
//! their plane layout and orientation, and the upright size `FrameConverter`
//! turns the last one into.
//!
//! On a physical device the case first needs camera access. iOS grants it
//! only through the system prompt, which a person answers once on the device;
//! there is no host-side grant like the simulator's `simctl privacy`. The case
//! asks for access and reports every state that keeps it from streaming —
//! denied, restricted, or a prompt nobody answered — as a failure naming the
//! step that is needed.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use waterkit::camera::{
    Camera, CameraConfig, CameraInfo, Frame, FrameConverter, FramePlanes, wgpu,
};
use waterkit::permission::{self, Permission, PermissionStatus};

use crate::{TestCase, TestReport};

/// Frames collected from each camera.
const FRAMES: usize = 5;

/// How long one camera has to deliver [`FRAMES`] frames once it is open.
const FRAME_TIMEOUT: Duration = Duration::from_secs(15);

/// How long the case waits for someone to answer the camera access prompt.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(60);

/// Lists the cameras, then streams [`FRAMES`] frames from each of them.
pub async fn record(report: &mut TestReport) {
    let cameras = match Camera::list() {
        Ok(cameras) => cameras,
        Err(error) => {
            report.push(TestCase::failed(
                "camera.list",
                format!("camera list failed: {error}"),
            ));
            return;
        }
    };

    if cameras.is_empty() {
        if cfg!(target_abi = "sim") {
            report.push(TestCase::passed_with_message(
                "camera.list",
                "count=0 (the simulator has no camera)",
            ));
        } else {
            report.push(TestCase::failed(
                "camera.list",
                "a physical device listed no cameras",
            ));
        }
        return;
    }

    report.push(TestCase::passed_with_message(
        "camera.list",
        format!("count={}", cameras.len()),
    ));

    if !camera_access(report).await {
        return;
    }

    let mut gpu = match Gpu::new().await {
        Ok(gpu) => gpu,
        Err(message) => {
            report.push(TestCase::failed("camera.gpu", message));
            return;
        }
    };

    for camera in &cameras {
        record_frames(report, &mut gpu, camera).await;
    }
}

/// Makes sure the app may use the camera, asking for it when nobody has
/// decided yet, and records the outcome as `camera.permission`.
async fn camera_access(report: &mut TestReport) -> bool {
    const CASE: &str = "camera.permission";

    let status = match permission::check(Permission::Camera).await {
        PermissionStatus::NotDetermined => {
            match tokio::time::timeout(PROMPT_TIMEOUT, permission::request(Permission::Camera))
                .await
            {
                Ok(Ok(status)) => status,
                Ok(Err(error)) => {
                    report.push(TestCase::failed(
                        CASE,
                        format!("requesting camera access failed: {error}"),
                    ));
                    return false;
                }
                Err(_) => {
                    report.push(TestCase::failed(
                        CASE,
                        format!(
                            "camera access is not determined: nobody answered the system prompt \
                             on the device within {PROMPT_TIMEOUT:?}, and it closes when the app \
                             exits. Run the harness again and tap Allow on the device while the \
                             prompt is showing"
                        ),
                    ));
                    return false;
                }
            }
        }
        status => status,
    };

    match status {
        PermissionStatus::Granted => {
            report.push(TestCase::passed_with_message(CASE, "status=Granted"));
            true
        }
        PermissionStatus::Denied => {
            report.push(TestCase::failed(
                CASE,
                "camera access is denied for this app: turn on Settings > Privacy & Security > \
                 Camera > WaterKitTest on the device, then run the harness again",
            ));
            false
        }
        status => {
            report.push(TestCase::failed(
                CASE,
                format!("camera access is {status:?}, so no camera can be opened"),
            ));
            false
        }
    }
}

/// The GPU device every camera imports its frames on, and the converter that
/// turns them upright.
struct Gpu {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    converter: FrameConverter,
}

impl Gpu {
    async fn new() -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .map_err(|error| format!("no GPU adapter: {error}"))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: FrameConverter::required_features(adapter.features()),
                ..Default::default()
            })
            .await
            .map_err(|error| format!("no GPU device: {error}"))?;
        Ok(Self {
            converter: FrameConverter::new(&device),
            device: Arc::new(device),
            queue: Arc::new(queue),
        })
    }
}

/// Opens `camera`, collects [`FRAMES`] frames and records what they look
/// like as `camera.frames.<id>`.
async fn record_frames(report: &mut TestReport, gpu: &mut Gpu, camera: &CameraInfo) {
    let case = format!("camera.frames.{}", camera.id);

    let handle = match Camera::open(
        &camera.id,
        CameraConfig::default(),
        Arc::clone(&gpu.device),
        Arc::clone(&gpu.queue),
    )
    .await
    {
        Ok(handle) => handle,
        Err(error) => {
            report.push(TestCase::failed(case, format!("open failed: {error}")));
            return;
        }
    };

    // Each frame is dropped before the next is taken: a frame holds a
    // capture buffer, and holding several starves the camera's pool.
    let mut frames = std::pin::pin!(handle.frames());
    let mut orientations = Vec::with_capacity(FRAMES);
    let mut last = None;
    for taken in 0..FRAMES {
        match tokio::time::timeout(FRAME_TIMEOUT, frames.next()).await {
            Ok(Some(Ok(frame))) => {
                orientations.push(frame.orientation());
                last = Some(frame);
            }
            Ok(Some(Err(error))) => {
                report.push(TestCase::failed(
                    case,
                    format!("stream failed after {taken} of {FRAMES} frames: {error}"),
                ));
                return;
            }
            Ok(None) => {
                report.push(TestCase::failed(
                    case,
                    format!("stream ended after {taken} of {FRAMES} frames"),
                ));
                return;
            }
            Err(_) => {
                report.push(TestCase::failed(
                    case,
                    format!(
                        "frame {} of {FRAMES} did not arrive within {FRAME_TIMEOUT:?}",
                        taken + 1
                    ),
                ));
                return;
            }
        }
    }
    let last = last.expect("FRAMES frames were taken");
    let upright = match gpu.converter.convert(&gpu.device, &gpu.queue, &last) {
        Ok(upright) => upright,
        Err(error) => {
            report.push(TestCase::failed(
                case,
                format!("frame conversion failed: {error}"),
            ));
            return;
        }
    };

    report.push(TestCase::passed_with_message(
        case,
        format!(
            "name={:?} front={} {} orientations={orientations:?} upright={}x{}",
            camera.name,
            camera.is_front_facing,
            describe(&last),
            upright.width(),
            upright.height(),
        ),
    ));
}

/// What the report says about a camera's last frame: its plane layout, with
/// the texture formats and encoding, and its stored size. This is the only
/// place the case reads a [`Frame`]'s planes.
fn describe(frame: &Frame) -> String {
    let layout = match frame.planes() {
        FramePlanes::Rgb(rgb) => format!("planes=rgb({:?})", rgb.texture().format()),
        FramePlanes::YCbCr420 { luma, chroma } => format!(
            "planes=ycbcr420(luma={:?}, chroma={:?}, color={:?})",
            luma.texture().format(),
            chroma.texture().format(),
            frame.color(),
        ),
        FramePlanes::YCbCr422 { yuyv } => format!(
            "planes=ycbcr422({:?}, color={:?})",
            yuyv.texture().format(),
            frame.color(),
        ),
    };
    format!("{layout} stored={}x{}", frame.width(), frame.height())
}
