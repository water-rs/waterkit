//! Camera case: streams frames from every camera on the device and reports
//! the layout the frames arrive in.
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
use waterkit::camera::{Camera, CameraConfig, CameraInfo, Frame, wgpu};
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

    let gpu = match Gpu::new().await {
        Ok(gpu) => gpu,
        Err(message) => {
            report.push(TestCase::failed("camera.gpu", message));
            return;
        }
    };

    for camera in &cameras {
        record_frames(report, &gpu, camera).await;
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

/// The GPU device every camera uploads its frames to.
struct Gpu {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
}

impl Gpu {
    async fn new() -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .map_err(|error| format!("no GPU adapter: {error}"))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .map_err(|error| format!("no GPU device: {error}"))?;
        Ok(Self {
            device: Arc::new(device),
            queue: Arc::new(queue),
        })
    }
}

/// Opens `camera`, collects [`FRAMES`] frames and records what they look
/// like as `camera.frames.<id>`.
async fn record_frames(report: &mut TestReport, gpu: &Gpu, camera: &CameraInfo) {
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

    let frames = handle.frames().take(FRAMES).collect::<Vec<_>>();
    let frames = match tokio::time::timeout(FRAME_TIMEOUT, frames).await {
        Ok(frames) if frames.len() == FRAMES => frames,
        Ok(frames) => {
            report.push(TestCase::failed(
                case,
                format!("stream ended after {} of {FRAMES} frames", frames.len()),
            ));
            return;
        }
        Err(_) => {
            report.push(TestCase::failed(
                case,
                format!("no {FRAMES} frames within {FRAME_TIMEOUT:?}"),
            ));
            return;
        }
    };

    report.push(TestCase::passed_with_message(
        case,
        format!(
            "name={:?} front={} {}",
            camera.name,
            camera.is_front_facing,
            describe(&frames)
        ),
    ));
}

/// What the report says about a camera's frames: the layout the last frame
/// is stored in and its size. This is the only place the case reads a
/// [`Frame`].
fn describe(frames: &[Frame]) -> String {
    let last = frames.last().expect("FRAMES frames were collected");
    format!(
        "format={:?} texture={:?} stored={}x{}",
        last.format(),
        last.texture().format(),
        last.width(),
        last.height(),
    )
}
