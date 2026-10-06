//! Android JNI generic test harness.

#![cfg(target_os = "android")]

use jni::errors::ThrowRuntimeExAndDefault;
#[cfg(feature = "location")]
use jni::objects::JDoubleArray;
use jni::objects::{Global, JObject};
use jni::sys::{jdoubleArray, jstring};
use jni::{Env, EnvUnowned};
#[cfg(feature = "clipboard")]
use waterkit_build::describe_jni_error;
use waterkit_test_report::{TestCase, TestReport, to_json_pretty};

const PERMISSION_NOT_DETERMINED: i32 = 0;
#[cfg(feature = "permission")]
const PERMISSION_RESTRICTED: i32 = 1;
#[cfg(feature = "permission")]
const PERMISSION_DENIED: i32 = 2;
#[cfg(feature = "permission")]
const PERMISSION_GRANTED: i32 = 3;
#[cfg(feature = "sensor")]
const ANDROID_SENSOR_TYPE_ACCELEROMETER: i32 = 1;

/// Runs the enabled cases and logs their report.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_waterkit_test_MainActivity_runTest<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    activity: JObject<'local>,
) {
    init_logger();
    env.with_env(|env| -> jni::errors::Result<()> {
        let _android_context = AndroidContextOwner::new(env, &activity)?;
        let report = run_native_report(env, &activity);
        log_report(&report);
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}

/// Runs the enabled cases and returns their report as JSON.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_waterkit_test_MainActivity_runTestReport<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    activity: JObject<'local>,
) -> jstring {
    init_logger();
    env.with_env(|env| -> jni::errors::Result<jstring> {
        let _android_context = AndroidContextOwner::new(env, &activity)?;
        let report = run_native_report(env, &activity);
        log_report(&report);

        let json = match to_json_pretty(&report) {
            Ok(json) => json,
            Err(error) => {
                log::error!("Failed to serialize WaterKit test report: {error}");
                return Ok(std::ptr::null_mut());
            }
        };

        match env.new_string(json) {
            Ok(value) => Ok(value.into_raw()),
            Err(error) => {
                log::error!("Failed to create Java report string: {error}");
                Ok(std::ptr::null_mut())
            }
        }
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

struct AndroidContextOwner {
    _activity: Global<JObject<'static>>,
}

impl AndroidContextOwner {
    fn new(env: &Env<'_>, activity: &JObject<'_>) -> jni::errors::Result<Self> {
        let java_vm = env.get_java_vm()?;
        let activity = env.new_global_ref(activity)?;
        // SAFETY: both pointers are retained for this owner's lifetime, and
        // the harness creates exactly one owner around each native test run.
        unsafe {
            ndk_context::initialize_android_context(
                java_vm.get_raw().cast(),
                activity.as_obj().as_raw().cast(),
            );
        }
        Ok(Self {
            _activity: activity,
        })
    }
}

impl Drop for AndroidContextOwner {
    fn drop(&mut self) {
        // SAFETY: construction initialized the context exactly once and this
        // owner is dropped exactly once after the native test run.
        unsafe {
            ndk_context::release_android_context();
        }
    }
}

fn init_logger() {
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );
    // A panic's message goes to stderr, which an Android app does not keep,
    // and the JNI boundary reports only that a panic happened; log it so a
    // crashed run says why.
    std::panic::set_hook(Box::new(|info| log::error!("Rust panic: {info}")));
}

fn run_native_report(env: &mut Env<'_>, activity: &JObject<'_>) -> TestReport {
    let mut report = TestReport::new("android", "waterkit-test-android");
    #[cfg(any(
        feature = "sensor",
        feature = "location",
        feature = "permission",
        feature = "fs",
        feature = "secret",
        feature = "clipboard"
    ))]
    let activity_global = match env.new_global_ref(activity) {
        Ok(value) => value,
        Err(error) => {
            report.push(TestCase::failed(
                "harness.activity_ref",
                format!("failed to create global activity ref: {error}"),
            ));
            return report;
        }
    };
    #[cfg(feature = "camera")]
    let files_dir = match files_dir(env, activity) {
        Ok(dir) => dir,
        Err(error) => {
            report.push(TestCase::failed("harness.files_dir", error.to_string()));
            return report;
        }
    };
    #[cfg(not(any(
        feature = "sensor",
        feature = "location",
        feature = "permission",
        feature = "fs",
        feature = "secret",
        feature = "camera",
        feature = "clipboard"
    )))]
    let _ = (env, activity);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime for Android test harness");

    rt.block_on(async {
        #[cfg(any(
            feature = "sensor",
            feature = "location",
            feature = "permission",
            feature = "fs",
            feature = "secret",
            feature = "clipboard"
        ))]
        let activity = activity_global.as_obj();

        #[cfg(feature = "sensor")]
        record_android_sensor(&mut report, env, activity);

        #[cfg(feature = "location")]
        record_android_location(&mut report, env, activity);

        #[cfg(feature = "permission")]
        record_android_permission(&mut report, env, activity);

        #[cfg(feature = "camera")]
        record_android_camera(&mut report, &files_dir).await;

        #[cfg(feature = "clipboard")]
        record_android_clipboard(&mut report).await;

        #[cfg(feature = "clipboard")]
        report.push(ClipboardFiles::record(env, activity).await);

        #[cfg(feature = "fs")]
        record_android_fs(&mut report, env, activity);

        #[cfg(feature = "haptic")]
        record_android_haptic(&mut report);

        #[cfg(feature = "notification")]
        record_android_notification(&mut report);

        #[cfg(feature = "secret")]
        record_android_secret(&mut report, env, activity);

        #[cfg(feature = "system")]
        record_android_system(&mut report);

        #[cfg(feature = "background")]
        record_android_background(&mut report);

        #[cfg(feature = "passkey")]
        record_android_passkey(&mut report).await;

        #[cfg(feature = "codec")]
        record_android_avif_decode(&mut report);

        #[cfg(feature = "health")]
        report.push(TestCase::passed_with_message(
            "health.availability",
            format!(
                "available={}",
                waterkit_content::health::capabilities().available
            ),
        ));

        #[cfg(feature = "screen")]
        record_android_screen(&mut report);

        for case in unexercised_cases() {
            report.push(case);
        }
    });

    // Every enabled feature records at least one case, so an empty report
    // means the harness was built without any feature.
    if report.cases.is_empty() {
        report.push(TestCase::failed(
            "harness.feature",
            "no WaterKit feature was enabled for the Android harness",
        ));
    }

    report
}

/// The cases of the features this harness only links, or cannot exercise
/// without an interactive prompt or the user's data.
fn unexercised_cases() -> impl Iterator<Item = TestCase> {
    [
        (
            cfg!(feature = "biometric"),
            TestCase::skipped(
                "biometric.authenticate",
                "biometric authentication requires an interactive prompt",
            ),
        ),
        (cfg!(feature = "audio"), TestCase::passed("audio.linked")),
        (cfg!(feature = "codec"), TestCase::passed("codec.linked")),
        (cfg!(feature = "dialog"), TestCase::passed("dialog.linked")),
        (cfg!(feature = "video"), TestCase::passed("video.linked")),
        (
            cfg!(feature = "bluetooth"),
            TestCase::passed("bluetooth.linked"),
        ),
        (cfg!(feature = "nfc"), TestCase::passed("nfc.linked")),
        (
            cfg!(feature = "share"),
            TestCase::skipped("share.sheet", "share sheet requires an interactive chooser"),
        ),
        (
            cfg!(feature = "speech"),
            TestCase::skipped(
                "speech.tts",
                "speech synthesis is audible and not asserted by this harness",
            ),
        ),
        (
            cfg!(feature = "contacts"),
            TestCase::skipped(
                "contacts.fetch_all",
                "contacts access depends on runtime user data permissions",
            ),
        ),
        (
            cfg!(feature = "calendar"),
            TestCase::skipped(
                "calendar.list",
                "calendar access depends on runtime user data permissions",
            ),
        ),
        (
            cfg!(feature = "deeplink"),
            TestCase::passed("deeplink.linked"),
        ),
    ]
    .into_iter()
    .filter_map(|(enabled, case)| enabled.then_some(case))
}

fn log_report(report: &TestReport) {
    log::info!(
        "WaterKit test report: platform={} crate={} passed={} skipped={} failed={}",
        report.platform,
        report.crate_name,
        report.passed_count(),
        report.skipped_count(),
        report.failed_count()
    );

    for case in &report.cases {
        log::info!(
            "case name={} status={:?} message={}",
            case.name,
            case.status,
            case.message.as_deref().unwrap_or("")
        );
    }
}

#[cfg(feature = "sensor")]
fn record_android_sensor(report: &mut TestReport, env: &mut Env<'_>, activity: &JObject<'_>) {
    match waterkit_content::sensor::android::is_sensor_available_with_context(
        env,
        activity,
        ANDROID_SENSOR_TYPE_ACCELEROMETER,
    ) {
        Ok(true) => {}
        Ok(false) => {
            report.push(TestCase::skipped(
                "sensor.accelerometer",
                "accelerometer is unavailable on this device",
            ));
            return;
        }
        Err(error) => {
            report.push(TestCase::failed(
                "sensor.accelerometer",
                format!("accelerometer availability check failed: {error}"),
            ));
            return;
        }
    }

    match waterkit_content::sensor::android::read_sensor_with_context(
        env,
        activity,
        ANDROID_SENSOR_TYPE_ACCELEROMETER,
    ) {
        Ok(data) if data.x().is_finite() && data.y().is_finite() && data.z().is_finite() => {
            report.push(TestCase::passed_with_message(
                "sensor.accelerometer",
                format!("x={:.3} y={:.3} z={:.3}", data.x(), data.y(), data.z()),
            ));
        }
        Ok(data) => report.push(TestCase::failed(
            "sensor.accelerometer",
            format!(
                "accelerometer returned non-finite sample x={} y={} z={}",
                data.x(),
                data.y(),
                data.z()
            ),
        )),
        Err(waterkit_content::sensor::SensorError::NotAvailable) => report.push(TestCase::skipped(
            "sensor.accelerometer",
            "accelerometer became unavailable before read",
        )),
        Err(error) => report.push(TestCase::failed(
            "sensor.accelerometer",
            format!("accelerometer reported available but read failed: {error}"),
        )),
    }
}

#[cfg(feature = "location")]
fn record_android_location(report: &mut TestReport, env: &mut Env<'_>, activity: &JObject<'_>) {
    match waterkit_content::location::android::provider_with_context(env, activity) {
        Ok(provider) => report.push(TestCase::passed_with_message(
            "location.provider",
            format!("{provider:?}"),
        )),
        Err(error) => report.push(TestCase::failed(
            "location.provider",
            format!("provider probe failed: {error}"),
        )),
    }
    match waterkit_content::location::android::get_location_with_context(env, activity) {
        Ok(location) => {
            let latitude = location.latitude().get();
            let longitude = location.longitude().get();
            if latitude.is_finite() && longitude.is_finite() {
                report.push(TestCase::passed_with_message(
                    "location.get",
                    format!("lat={latitude:.6} lon={longitude:.6}"),
                ));
            } else {
                report.push(TestCase::failed(
                    "location.get",
                    format!(
                        "location contained non-finite coordinates lat={latitude} lon={longitude}"
                    ),
                ));
            }
        }
        Err(waterkit_content::location::LocationError::NotAvailable) => report.push(
            TestCase::skipped("location.get", "Android has no last known location"),
        ),
        Err(error) => report.push(TestCase::failed(
            "location.get",
            format!("location read failed: {error}"),
        )),
    }
}

#[cfg(feature = "permission")]
fn record_android_permission(report: &mut TestReport, env: &mut Env<'_>, activity: &JObject<'_>) {
    match waterkit_content::permission::android::check_with_activity(
        env,
        activity,
        waterkit_content::permission::Permission::Location,
    ) {
        Ok(status) => report.push(TestCase::passed_with_message(
            "permission.location",
            format!("status={status:?}"),
        )),
        Err(error) => report.push(TestCase::failed(
            "permission.location",
            format!("permission check failed: {error}"),
        )),
    }
}

/// The activity's private files directory, where the runner pulls artifacts
/// from with `run-as`.
#[cfg(feature = "camera")]
fn files_dir(env: &mut Env<'_>, activity: &JObject<'_>) -> jni::errors::Result<std::path::PathBuf> {
    use jni::{jni_sig, jni_str};
    let dir = env
        .call_method(
            activity,
            jni_str!("getFilesDir"),
            jni_sig!("()Ljava/io/File;"),
            &[],
        )?
        .l()?;
    let path = env
        .call_method(
            &dir,
            jni_str!("getAbsolutePath"),
            jni_sig!("()Ljava/lang/String;"),
            &[],
        )?
        .l()?;
    let path = env
        .as_cast::<jni::objects::JString>(&path)?
        .try_to_string(env)?;
    Ok(std::path::PathBuf::from(path))
}

#[cfg(feature = "camera")]
async fn record_android_camera(report: &mut TestReport, files_dir: &std::path::Path) {
    match waterkit_content::camera::Camera::list() {
        Ok(cameras) => {
            report.push(TestCase::passed_with_message(
                "camera.list",
                format!("count={}", cameras.len()),
            ));
            for camera in cameras {
                record_android_camera_frames(report, &camera, files_dir).await;
            }
        }
        Err(error) => report.push(TestCase::failed(
            "camera.list",
            format!("camera list failed: {error}"),
        )),
    }
}

/// Streams `camera` for a few seconds, dropping each frame once it is
/// converted, and reports the plane layouts, the orientations, and the frame
/// rate; the last frame, converted upright on the GPU, is saved as
/// `camera-<id>.png` in the files directory for inspection.
#[cfg(feature = "camera")]
async fn record_android_camera_frames(
    report: &mut TestReport,
    camera: &waterkit_content::camera::CameraInfo,
    files_dir: &std::path::Path,
) {
    use futures::StreamExt;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use waterkit_content::camera::{Camera, CameraConfig, FrameConverter, wgpu};

    const STREAM: Duration = Duration::from_secs(3);
    let case = format!("camera.frames.{}", camera.id);

    let (device, queue) = match camera_gpu().await {
        Ok(gpu) => gpu,
        Err(error) => {
            report.push(TestCase::failed(case, error));
            return;
        }
    };
    let camera_handle = match Camera::open(
        &camera.id,
        CameraConfig::default(),
        Arc::clone(&device),
        Arc::clone(&queue),
    )
    .await
    {
        Ok(handle) => handle,
        Err(error) => {
            report.push(TestCase::failed(case, format!("open failed: {error}")));
            return;
        }
    };

    let mut converter = FrameConverter::new(&device);
    let mut frames = std::pin::pin!(camera_handle.frames());
    let mut summary = FrameSummary::default();
    let mut upright = None;
    let started = Instant::now();
    while started.elapsed() < STREAM {
        let next = tokio::time::timeout(Duration::from_secs(5), frames.next()).await;
        let frame = match next_frame(&case, summary.count, next) {
            Ok(frame) => frame,
            Err(outcome) => {
                report.push(outcome);
                return;
            }
        };
        summary.record(&frame);
        let output = upright
            .take()
            .filter(|texture: &wgpu::Texture| {
                texture.size() == FrameConverter::upright_size(&frame)
            })
            .unwrap_or_else(|| FrameConverter::create_output(&device, &frame));
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        if let Err(error) = converter.encode(&device, &mut encoder, &frame, &output) {
            report.push(TestCase::failed(
                case,
                format!("frame conversion failed: {error}"),
            ));
            return;
        }
        queue.submit([encoder.finish()]);
        upright = Some(output);
    }
    let elapsed = started.elapsed().as_secs_f64();
    let upright = upright.expect("at least one frame was converted");
    let png = files_dir.join(format!("camera-{}.png", camera.id));
    if let Err(error) = save_png(&device, &queue, &upright, &png) {
        report.push(TestCase::failed(
            case,
            format!("saving {}: {error}", png.display()),
        ));
        return;
    }
    let FrameSummary {
        layouts,
        orientations,
        count,
        stored,
    } = summary;
    report.push(TestCase::passed_with_message(
        case,
        format!(
            "front={} frames={count} fps={:.1} planes={layouts:?} stored={}x{} orientations={orientations:?} upright={}x{} png={}",
            camera.is_front_facing,
            f64::from(count) / elapsed,
            stored.0,
            stored.1,
            upright.width(),
            upright.height(),
            png.display(),
        ),
    ));
}

/// The Vulkan device camera frames are imported on: Android camera frames are
/// `AHardwareBuffer`s, which only Vulkan can take, so the device carries the
/// import's extensions and NV12, which drivers that map camera buffers to a
/// Vulkan format alias them as.
#[cfg(feature = "camera")]
async fn camera_gpu() -> Result<
    (
        std::sync::Arc<waterkit_content::camera::wgpu::Device>,
        std::sync::Arc<waterkit_content::camera::wgpu::Queue>,
    ),
    String,
> {
    use std::sync::Arc;
    use waterkit_content::camera::wgpu_external_frame::ahardware_buffer;
    use waterkit_content::camera::{FrameConverter, wgpu};

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .map_err(|error| format!("no Vulkan adapter: {error}"))?;
    let features =
        FrameConverter::required_features(adapter.features()) | wgpu::Features::TEXTURE_FORMAT_NV12;
    let (device, queue) = ahardware_buffer::request_device(
        &adapter,
        &wgpu::DeviceDescriptor {
            required_features: features,
            ..Default::default()
        },
    )
    .map_err(|error| format!("no GPU device: {error}"))?;
    Ok((Arc::new(device), Arc::new(queue)))
}

/// The next frame of a camera stream, or the outcome that ends the case after
/// `count` frames: skipped when this GPU cannot convert the camera's
/// driver-private buffers, failed for any other end of the stream.
#[cfg(feature = "camera")]
fn next_frame(
    case: &str,
    count: u32,
    next: Result<
        Option<Result<waterkit_content::camera::Frame, waterkit_content::camera::CameraError>>,
        tokio::time::error::Elapsed,
    >,
) -> Result<waterkit_content::camera::Frame, TestCase> {
    use waterkit_content::camera::CameraError;
    use waterkit_content::camera::wgpu_external_frame::ahardware_buffer::HardwareBufferImportError;

    match next {
        Ok(Some(Ok(frame))) => Ok(frame),
        Ok(Some(Err(CameraError::FrameImport(error))))
            if matches!(
                *error,
                HardwareBufferImportError::ConversionUnavailable { .. }
            ) =>
        {
            Err(TestCase::skipped(
                case,
                format!("after {count} frames: {error}"),
            ))
        }
        Ok(Some(Err(error))) => Err(TestCase::failed(
            case,
            format!("stream failed after {count} frames: {error}"),
        )),
        Ok(None) => Err(TestCase::failed(
            case,
            format!("stream ended after {count} frames"),
        )),
        Err(_) => Err(TestCase::failed(
            case,
            format!("no frame within 5 s after {count}"),
        )),
    }
}

/// What a camera's frames showed while the harness streamed them.
#[cfg(feature = "camera")]
#[derive(Default)]
struct FrameSummary {
    /// Plane layouts, matrices and ranges seen.
    layouts: std::collections::BTreeSet<&'static str>,
    orientations: std::collections::BTreeSet<String>,
    count: u32,
    /// The stored size of the last frame.
    stored: (u32, u32),
}

#[cfg(feature = "camera")]
impl FrameSummary {
    fn record(&mut self, frame: &waterkit_content::camera::Frame) {
        use waterkit_content::camera::{FramePlanes, MatrixCoefficients};

        self.count += 1;
        self.layouts.insert(match frame.planes() {
            FramePlanes::Rgb(_) => "rgb",
            FramePlanes::YCbCr420 { .. } => "ycbcr420",
            FramePlanes::YCbCr422 { .. } => "ycbcr422",
        });
        if matches!(
            frame.planes(),
            FramePlanes::YCbCr420 { .. } | FramePlanes::YCbCr422 { .. }
        ) {
            self.layouts.insert(match frame.color().matrix {
                MatrixCoefficients::Bt601 => "bt601",
                MatrixCoefficients::Bt709 => "bt709",
                MatrixCoefficients::Bt2020NonConstantLuminance => "bt2020",
                MatrixCoefficients::Bt2020ConstantLuminance => "bt2020-constant-luminance",
            });
            self.layouts.insert(match frame.color().range {
                waterkit_content::camera::ColorRange::Limited => "video-range",
                waterkit_content::camera::ColorRange::Full => "full-range",
            });
        }
        self.orientations
            .insert(format!("{:?}", frame.orientation()));
        self.stored = (frame.width(), frame.height());
    }
}

/// Reads an upright `Rgba8Unorm` frame back and writes it as a PNG; the
/// readback is test tooling, not part of the camera path.
#[cfg(feature = "camera")]
fn save_png(
    device: &waterkit_content::camera::wgpu::Device,
    queue: &waterkit_content::camera::wgpu::Queue,
    texture: &waterkit_content::camera::wgpu::Texture,
    path: &std::path::Path,
) -> Result<(), String> {
    use waterkit_content::camera::wgpu;
    let size = texture.size();
    let row = size.width * 4;
    let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("harness png readback"),
        size: u64::from(padded * size.height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(size.height),
            },
        },
        size,
    );
    queue.submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(10)),
        })
        .map_err(|error| error.to_string())?;
    let mapped = buffer
        .slice(..)
        .get_mapped_range()
        .map_err(|error| error.to_string())?;
    let pixels: Vec<u8> = mapped
        .chunks(padded as usize)
        .flat_map(|line| &line[..row as usize])
        .copied()
        .collect();
    image::RgbaImage::from_raw(size.width, size.height, pixels)
        .ok_or("readback size")?
        .save(path)
        .map_err(|error| error.to_string())
}

#[cfg(feature = "clipboard")]
async fn record_android_clipboard(report: &mut TestReport) {
    let mut clipboard = match waterkit_content::clipboard::Clipboard::new() {
        Ok(clipboard) => clipboard,
        Err(error) => {
            report.push(TestCase::failed(
                "clipboard.init",
                format!("clipboard init failed: {error}"),
            ));
            return;
        }
    };

    if let Err(error) = clipboard.set_text("WaterKit Test") {
        report.push(TestCase::failed(
            "clipboard.set_text",
            format!("set_text failed: {error}"),
        ));
        return;
    }

    match clipboard.has_text() {
        Ok(true) => report.push(TestCase::passed("clipboard.has_text")),
        Ok(false) => report.push(TestCase::failed(
            "clipboard.has_text",
            "has_text reported no text right after set_text",
        )),
        Err(error) => report.push(TestCase::failed(
            "clipboard.has_text",
            format!("has_text failed: {error}"),
        )),
    }

    match clipboard.text().await {
        Ok(text) if text.as_deref() == Some("WaterKit Test") => {
            report.push(TestCase::passed("clipboard.round_trip"));
        }
        Ok(_text) => report.push(TestCase::failed(
            "clipboard.round_trip",
            "round-trip text did not match the synthetic clip (contents not printed)",
        )),
        Err(error) => report.push(TestCase::failed(
            "clipboard.round_trip",
            format!("get_text failed: {error}"),
        )),
    }

    record_android_clipboard_watch(report, &mut clipboard).await;
}

/// The files the `clipboard.files_round_trip` case copies: two in the app's
/// cache, one with a name every URL encoding must escape.
#[cfg(feature = "clipboard")]
struct ClipboardFiles {
    /// The clip's first URI, as `ClipboardFileProvider` builds it.
    expected_uri: String,
    paths: Vec<std::path::PathBuf>,
}

#[cfg(feature = "clipboard")]
impl ClipboardFiles {
    const NAME: &str = "waterkit clipboard n\u{e4}me #1?.txt";
    const ENCODED_NAME: &str = "waterkit%20clipboard%20n%C3%A4me%20%231%3F.txt";
    const CONTENTS: &[u8] = b"WaterKit clipboard file\n";

    /// Runs the case.
    #[expect(
        clippy::future_not_send,
        reason = "the JNI environment belongs to the harness thread, which the current-thread runtime blocks on"
    )]
    async fn record(env: &mut Env<'_>, activity: &JObject<'_>) -> TestCase {
        const CASE: &str = "clipboard.files_round_trip";
        let files = match Self::new(env, activity) {
            Ok(files) => files,
            Err(error) => return TestCase::failed(CASE, error),
        };
        if let Err(error) = files.round_trip().await {
            return TestCase::failed(CASE, error);
        }
        match files.check_clip(env, activity) {
            Ok(uri) => TestCase::passed_with_message(CASE, format!("uri={uri}")),
            Err(error) => TestCase::failed(CASE, error),
        }
    }

    /// The files in `activity`'s cache directory.
    fn new(env: &mut Env<'_>, activity: &JObject<'_>) -> Result<Self, String> {
        use jni::{jni_sig, jni_str};

        let cache_dir = call_object(
            env,
            activity,
            jni_str!("getCacheDir"),
            &jni_sig!("()Ljava/io/File;"),
            &[],
        )?;
        let cache_dir = call_object(
            env,
            &cache_dir,
            jni_str!("getAbsolutePath"),
            &jni_sig!("()Ljava/lang/String;"),
            &[],
        )?;
        let cache_dir = std::path::PathBuf::from(java_string(env, &cache_dir)?);
        let package = call_object(
            env,
            activity,
            jni_str!("getPackageName"),
            &jni_sig!("()Ljava/lang/String;"),
            &[],
        )?;
        let package = java_string(env, &package)?;
        Ok(Self {
            expected_uri: format!(
                "content://{package}.waterkit.clipboard{}/{}",
                cache_dir.display(),
                Self::ENCODED_NAME
            ),
            paths: vec![cache_dir.join(Self::NAME), cache_dir.join("plain.txt")],
        })
    }

    /// Copies the files, and reads back their paths and the first one's
    /// contents through the clipboard.
    async fn round_trip(&self) -> Result<(), String> {
        for path in &self.paths {
            std::fs::write(path, Self::CONTENTS)
                .map_err(|error| format!("writing {}: {error}", path.display()))?;
        }
        let mut clipboard = waterkit_content::clipboard::Clipboard::new()
            .map_err(|error| format!("clipboard init failed: {error}"))?;
        clipboard
            .set_files(&self.paths)
            .map_err(|error| format!("set_files failed: {error}"))?;
        let read = clipboard
            .files()
            .await
            .map_err(|error| format!("files failed: {error}"))?;
        if read != self.paths {
            return Err(format!("files() returned {read:?} for {:?}", self.paths));
        }
        let contents = clipboard
            .binary("text/plain")
            .await
            .map_err(|error| format!("reading the first file failed: {error}"))?;
        if contents.as_deref() != Some(Self::CONTENTS) {
            return Err(format!("the first file's URI served {contents:?}"));
        }
        Ok(())
    }

    /// Checks the clip's first URI, and grants `com.android.shell` read
    /// access to it, as the clipboard grants the app that reads the clip, so
    /// that `adb shell content read --uri <uri>` can open it as another app.
    fn check_clip(&self, env: &mut Env<'_>, activity: &JObject<'_>) -> Result<String, String> {
        use jni::objects::JValue;
        use jni::{jni_sig, jni_str};

        const FLAG_GRANT_READ_URI_PERMISSION: i32 = 1;

        let service = env
            .new_string("clipboard")
            .map_err(|error| describe_jni_error(env, error))?;
        let manager = call_object(
            env,
            activity,
            jni_str!("getSystemService"),
            &jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"),
            &[JValue::Object(&service)],
        )?;
        let clip = call_object(
            env,
            &manager,
            jni_str!("getPrimaryClip"),
            &jni_sig!("()Landroid/content/ClipData;"),
            &[],
        )?;
        let item = call_object(
            env,
            &clip,
            jni_str!("getItemAt"),
            &jni_sig!("(I)Landroid/content/ClipData$Item;"),
            &[JValue::Int(0)],
        )?;
        let uri = call_object(
            env,
            &item,
            jni_str!("getUri"),
            &jni_sig!("()Landroid/net/Uri;"),
            &[],
        )?;
        let uri_text = call_object(
            env,
            &uri,
            jni_str!("toString"),
            &jni_sig!("()Ljava/lang/String;"),
            &[],
        )?;
        let uri_text = java_string(env, &uri_text)?;
        if uri_text != self.expected_uri {
            return Err(format!(
                "the clip names {uri_text}, expected {}",
                self.expected_uri
            ));
        }

        let shell = env
            .new_string("com.android.shell")
            .map_err(|error| describe_jni_error(env, error))?;
        env.call_method(
            activity,
            jni_str!("grantUriPermission"),
            jni_sig!("(Ljava/lang/String;Landroid/net/Uri;I)V"),
            &[
                JValue::Object(&shell),
                JValue::Object(&uri),
                JValue::Int(FLAG_GRANT_READ_URI_PERMISSION),
            ],
        )
        .map_err(|error| format!("grantUriPermission: {}", describe_jni_error(env, error)))?;
        Ok(uri_text)
    }
}

/// Calls the object-returning method `name` of `object`.
#[cfg(feature = "clipboard")]
fn call_object<'local>(
    env: &mut Env<'local>,
    object: &JObject<'_>,
    name: &'static jni::strings::JNIStr,
    signature: &jni::signature::MethodSignature<'_, '_>,
    args: &[jni::objects::JValue<'_>],
) -> Result<JObject<'local>, String> {
    env.call_method(object, name, signature, args)
        .and_then(jni::JValueOwned::l)
        .map_err(|error| format!("{name}: {}", describe_jni_error(env, error)))
}

/// The contents of the `java.lang.String` `value`.
#[cfg(feature = "clipboard")]
fn java_string(env: &Env<'_>, value: &JObject<'_>) -> Result<String, String> {
    waterkit_build::decode_string(env, value).map_err(|error| error.to_string())
}

/// Outcome of waiting on a clipboard stream, with a bound so a broken
/// watcher fails the case instead of hanging the harness.
#[cfg(feature = "clipboard")]
enum ClipWait {
    Event(waterkit_content::clipboard::ClipboardEvent),
    Closed,
    TimedOut,
}

#[cfg(feature = "clipboard")]
impl std::fmt::Debug for ClipWait {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Event(event) => f.debug_tuple("Event").field(event).finish(),
            Self::Closed => f.write_str("Closed"),
            Self::TimedOut => f.write_str("TimedOut"),
        }
    }
}

#[cfg(feature = "clipboard")]
async fn next_clipboard_event(
    stream: &mut waterkit_content::clipboard::ClipboardStream,
) -> ClipWait {
    use futures::StreamExt;

    match tokio::time::timeout(std::time::Duration::from_secs(10), stream.next()).await {
        Ok(Some(event)) => ClipWait::Event(event),
        Ok(None) => ClipWait::Closed,
        Err(_) => ClipWait::TimedOut,
    }
}

#[cfg(feature = "clipboard")]
async fn record_android_clipboard_watch(
    report: &mut TestReport,
    clipboard: &mut waterkit_content::clipboard::Clipboard,
) {
    const FIRST: &str = "WaterKit Watch First";
    const SECOND: &str = "WaterKit Watch Second";

    let mut primary_stream = match clipboard.watch() {
        Ok(stream) => stream,
        Err(error) => {
            report.push(TestCase::failed(
                "clipboard.watch_start",
                format!("watch failed: {error}"),
            ));
            return;
        }
    };
    // A second subscriber on the same clipboard: watchers must be
    // independent, each registering its own listener.
    let mut second_stream = match clipboard.watch() {
        Ok(stream) => stream,
        Err(error) => {
            primary_stream.stop();
            report.push(TestCase::failed(
                "clipboard.watch_start",
                format!("second watch failed: {error}"),
            ));
            return;
        }
    };

    // Two successive same-type writes must produce two separate events on
    // every subscriber. Regression coverage for waterkit#113: the old
    // polling watcher only emitted when the type-presence bitmask changed,
    // so the second text write never reached the stream.
    if let Err(error) = clipboard.set_text(FIRST) {
        report.push(TestCase::failed(
            "clipboard.watch_same_type",
            format!("set_text failed: {error}"),
        ));
        return;
    }
    let (primary_first, second_first) = tokio::join!(
        next_clipboard_event(&mut primary_stream),
        next_clipboard_event(&mut second_stream)
    );

    if let Err(error) = clipboard.set_text(SECOND) {
        report.push(TestCase::failed(
            "clipboard.watch_same_type",
            format!("set_text failed: {error}"),
        ));
        return;
    }
    let (primary_second, second_second) = tokio::join!(
        next_clipboard_event(&mut primary_stream),
        next_clipboard_event(&mut second_stream)
    );

    record_two_text_events(
        report,
        "clipboard.watch_same_type",
        primary_first,
        primary_second,
    );
    record_two_text_events(
        report,
        "clipboard.watch_independent_subscribers",
        second_first,
        second_second,
    );

    // Dropping a stream must unregister its listener and release the
    // callback state without a use-after-free; a fresh watcher receiving a
    // clip event proves the clipboard stays observable through the same
    // callback path afterwards.
    drop(second_stream);
    match clipboard.watch() {
        Ok(mut fresh_stream) => {
            if let Err(error) = clipboard.set_text(FIRST) {
                report.push(TestCase::failed(
                    "clipboard.watch_drop",
                    format!("set_text failed: {error}"),
                ));
                return;
            }
            match next_clipboard_event(&mut fresh_stream).await {
                ClipWait::Event(event) if event.has_text() => {
                    report.push(TestCase::passed("clipboard.watch_drop"));
                }
                wait => report.push(TestCase::failed(
                    "clipboard.watch_drop",
                    format!("fresh watcher after drop produced {wait:?}"),
                )),
            }
        }
        Err(error) => report.push(TestCase::failed(
            "clipboard.watch_drop",
            format!("watch after drop failed: {error}"),
        )),
    }

    record_watch_stop(report, clipboard, primary_stream).await;
}

/// Records a case that passes when both waits produced a text event.
#[cfg(feature = "clipboard")]
fn record_two_text_events(
    report: &mut TestReport,
    name: &'static str,
    first: ClipWait,
    second: ClipWait,
) {
    match (first, second) {
        (ClipWait::Event(first), ClipWait::Event(second))
            if first.has_text() && second.has_text() =>
        {
            report.push(TestCase::passed(name));
        }
        (first, second) => report.push(TestCase::failed(
            name,
            format!("expected two text events, got {first:?} then {second:?}"),
        )),
    }
}

#[cfg(feature = "clipboard")]
async fn record_watch_stop(
    report: &mut TestReport,
    clipboard: &mut waterkit_content::clipboard::Clipboard,
    mut primary_stream: waterkit_content::clipboard::ClipboardStream,
) {
    const AFTER_LIFECYCLE: &str = "WaterKit Watch After Lifecycle";

    // stop() unregisters the listener and releases the callback state; the
    // channel then completes once events buffered before the stop have
    // drained — async-channel's documented termination — so the wait ends
    // only on `Closed`, not on an arbitrary event budget.
    primary_stream.stop();
    if let Err(error) = clipboard.set_text(AFTER_LIFECYCLE) {
        report.push(TestCase::failed(
            "clipboard.watch_stop",
            format!("set_text failed: {error}"),
        ));
        return;
    }
    let mut wait = next_clipboard_event(&mut primary_stream).await;
    while matches!(wait, ClipWait::Event(_)) {
        wait = next_clipboard_event(&mut primary_stream).await;
    }
    if matches!(wait, ClipWait::Closed) {
        report.push(TestCase::passed("clipboard.watch_stop"));
    } else {
        report.push(TestCase::failed(
            "clipboard.watch_stop",
            format!("channel ended with {wait:?}, not Closed"),
        ));
    }
}

#[cfg(feature = "fs")]
fn record_android_fs(report: &mut TestReport, env: &mut Env<'_>, activity: &JObject<'_>) {
    match waterkit_content::fs::WaterFs::cache_dir_with_context(env, activity) {
        Ok(path) if path.as_os_str().is_empty() => report.push(TestCase::failed(
            "fs.cache_dir",
            "cache directory path was empty",
        )),
        Ok(path) => report.push(TestCase::passed_with_message(
            "fs.cache_dir",
            format!("path={}", path.display()),
        )),
        Err(error) => report.push(TestCase::failed(
            "fs.cache_dir",
            format!("cache_dir failed: {error}"),
        )),
    }
}

#[cfg(feature = "haptic")]
fn record_android_haptic(report: &mut TestReport) {
    match waterkit_content::haptic::Haptic::impact(waterkit_content::haptic::Intensity::LOW) {
        Ok(()) => report.push(TestCase::passed("haptic.impact")),
        Err(error) => report.push(TestCase::failed(
            "haptic.impact",
            format!("haptic impact failed: {error}"),
        )),
    }
}

#[cfg(feature = "notification")]
fn record_android_notification(report: &mut TestReport) {
    let result = waterkit_content::notification::Notification::new()
        .title("WaterKit Android Harness")
        .body("notification test")
        .show();
    match result {
        Ok(_) => report.push(TestCase::passed("notification.show")),
        Err(error) => report.push(TestCase::failed(
            "notification.show",
            format!("notification show failed: {error}"),
        )),
    }
}

#[cfg(feature = "secret")]
fn record_android_secret(report: &mut TestReport, env: &mut Env<'_>, activity: &JObject<'_>) {
    match waterkit_content::secret::android::set_with_context(
        env,
        activity,
        "waterkit",
        "test",
        "secret123",
    ) {
        Ok(()) => {}
        Err(error) => {
            // The keystore deliberately requires hardware-backed keys; the
            // emulator only offers a software keystore.
            if error.to_string().contains("hardware-backed") {
                for case in ["secret.set", "secret.get", "secret.delete"] {
                    report.push(TestCase::skipped(
                        case,
                        "emulator keystore is not hardware-backed",
                    ));
                }
            } else {
                report.push(TestCase::failed(
                    "secret.set",
                    format!("secret set failed: {error}"),
                ));
            }
            return;
        }
    }

    match waterkit_content::secret::android::get_with_context(env, activity, "waterkit", "test") {
        Ok(value) if value == "secret123" => report.push(TestCase::passed("secret.get")),
        Ok(value) => report.push(TestCase::failed(
            "secret.get",
            format!("expected secret123, got {value:?}"),
        )),
        Err(error) => report.push(TestCase::failed(
            "secret.get",
            format!("secret get failed: {error}"),
        )),
    }

    match waterkit_content::secret::android::delete_with_context(env, activity, "waterkit", "test")
    {
        Ok(()) => report.push(TestCase::passed("secret.delete")),
        Err(error) => report.push(TestCase::failed(
            "secret.delete",
            format!("secret delete failed: {error}"),
        )),
    }
}

#[cfg(feature = "system")]
fn record_android_system(report: &mut TestReport) {
    use waterkit_content::system;

    report.push(match system::connectivity() {
        Ok(info) => TestCase::passed_with_message(
            "system.connectivity",
            format!(
                "type={:?} connected={}",
                info.connection_type(),
                info.is_connected()
            ),
        ),
        Err(error) => TestCase::failed("system.connectivity", error.to_string()),
    });
    report.push(match system::thermal_state() {
        Ok(state) => TestCase::passed_with_message("system.thermal_state", format!("{state:?}")),
        Err(error) => TestCase::failed("system.thermal_state", error.to_string()),
    });
    report.push(match system::load() {
        Ok(load) if load.memory_total() > 0 && load.memory_used() <= load.memory_total() => {
            TestCase::passed_with_message("system.load", format!("{load:?}"))
        }
        Ok(load) => TestCase::failed("system.load", format!("implausible memory: {load:?}")),
        Err(error) => TestCase::failed("system.load", error.to_string()),
    });
}

#[cfg(feature = "background")]
fn record_android_background(report: &mut TestReport) {
    let capabilities = waterkit_content::background::capabilities();
    report.push(TestCase::passed_with_message(
        "background.capabilities",
        format!(
            "refresh={} processing={} continued={} launch_events={}",
            capabilities.supports_app_refresh,
            capabilities.supports_processing,
            capabilities.supports_continued_processing,
            capabilities.supports_launch_events
        ),
    ));
}

#[cfg(feature = "passkey")]
async fn record_android_passkey(report: &mut TestReport) {
    match waterkit_content::passkey::is_available().await {
        Ok(availability) if availability.is_platform_supported => {
            report.push(TestCase::passed_with_message(
                "passkey.availability",
                format!(
                    "supported=true user_verification={} discoverable={}",
                    availability.supports_user_verification,
                    availability.supports_discoverable_credentials
                ),
            ));
        }
        Ok(_) => report.push(TestCase::failed(
            "passkey.availability",
            "passkey reports unsupported on an API 34+ CredentialManager device",
        )),
        Err(error) => report.push(TestCase::failed(
            "passkey.availability",
            format!("passkey availability failed: {error}"),
        )),
    }
}

#[cfg(feature = "screen")]
fn record_android_screen(report: &mut TestReport) {
    match waterkit_content::screen::screens() {
        Ok(screens) => report.push(TestCase::passed_with_message(
            "screen.list",
            format!("count={}", screens.len()),
        )),
        Err(error) => report.push(TestCase::failed(
            "screen.list",
            format!("screen enumeration failed: {error}"),
        )),
    }
}

/// Checks one permission and returns its status code.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_waterkit_test_MainActivity_testCheckPermission<'local>(
    mut unownedenv: EnvUnowned<'local>,
    _this: JObject<'local>,
    activity: JObject<'local>,
    permission_type: i32,
) -> i32 {
    unownedenv
        .with_env(|env| -> jni::errors::Result<i32> {
            Ok(check_permission(env, &activity, permission_type))
        })
        .resolve::<ThrowRuntimeExAndDefault>()
}

fn check_permission(env: &mut Env<'_>, activity: &JObject<'_>, permission_type: i32) -> i32 {
    #[cfg(feature = "permission")]
    {
        let permission = match permission_type {
            0 => waterkit_content::permission::Permission::Location,
            1 => waterkit_content::permission::Permission::Camera,
            2 => waterkit_content::permission::Permission::Microphone,
            3 => waterkit_content::permission::Permission::Photos,
            4 => waterkit_content::permission::Permission::Contacts,
            5 => waterkit_content::permission::Permission::Calendar,
            _ => {
                log::error!("Unknown permission type: {permission_type}");
                return PERMISSION_NOT_DETERMINED;
            }
        };

        match waterkit_content::permission::android::check_with_activity(env, activity, permission)
        {
            Ok(waterkit_content::permission::PermissionStatus::NotDetermined) => {
                PERMISSION_NOT_DETERMINED
            }
            Ok(waterkit_content::permission::PermissionStatus::Restricted) => PERMISSION_RESTRICTED,
            Ok(waterkit_content::permission::PermissionStatus::Denied) => PERMISSION_DENIED,
            Ok(waterkit_content::permission::PermissionStatus::Granted) => PERMISSION_GRANTED,
            Ok(status) => {
                log::error!("Unknown permission status: {status:?}");
                PERMISSION_NOT_DETERMINED
            }
            Err(error) => {
                log::error!("Permission check failed: {error}");
                PERMISSION_NOT_DETERMINED
            }
        }
    }

    #[cfg(not(feature = "permission"))]
    {
        let _ = (env, activity, permission_type);
        log::error!("testCheckPermission called without enabling permission feature");
        PERMISSION_NOT_DETERMINED
    }
}

/// Reads the location as `[ok, latitude, longitude, altitude, accuracy]`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_waterkit_test_MainActivity_testGetLocation<'local>(
    mut unownedenv: EnvUnowned<'local>,
    _this: JObject<'local>,
    activity: JObject<'local>,
) -> jdoubleArray {
    unownedenv
        .with_env(|env| -> jni::errors::Result<jdoubleArray> { Ok(get_location(env, &activity)) })
        .resolve::<ThrowRuntimeExAndDefault>()
}

fn get_location(env: &mut Env<'_>, activity: &JObject<'_>) -> jdoubleArray {
    #[cfg(feature = "location")]
    {
        match waterkit_content::location::android::get_location_with_context(env, activity) {
            Ok(location) => {
                let altitude = location.altitude().unwrap_or(0.0);
                let accuracy = location.horizontal_accuracy().unwrap_or(0.0);
                let payload = [
                    1.0,
                    location.latitude().get(),
                    location.longitude().get(),
                    altitude,
                    accuracy,
                ];

                let array = match JDoubleArray::new(env, payload.len()) {
                    Ok(arr) => arr,
                    Err(error) => {
                        log::error!("JDoubleArray::new failed: {error}");
                        return std::ptr::null_mut();
                    }
                };

                if let Err(error) = array.set_region(env, 0, &payload) {
                    log::error!("set_region failed: {error}");
                    return std::ptr::null_mut();
                }

                array.into_raw()
            }
            Err(error) => {
                log::error!("Location test failed: {error}");
                std::ptr::null_mut()
            }
        }
    }

    #[cfg(not(feature = "location"))]
    {
        let _ = (env, activity);
        log::error!("testGetLocation called without enabling location feature");
        std::ptr::null_mut()
    }
}

#[cfg(feature = "codec")]
fn record_android_avif_decode(report: &mut TestReport) {
    const AVIF: &[u8] = include_bytes!("../fixtures/quadrants.avif");
    match waterkit_content::codec::decode_image(AVIF) {
        Ok(image) => {
            let pixels = image.pixels();
            if image.width() != 8 || image.height() != 8 {
                report.push(TestCase::failed(
                    "codec.decode_avif_platform",
                    format!("expected 8x8, got {}x{}", image.width(), image.height()),
                ));
                return;
            }
            if image.pixel_format() != waterkit_content::codec::DecodedPixelFormat::Rgba8UnormSrgb {
                report.push(TestCase::failed(
                    "codec.decode_avif_platform",
                    format!("unexpected pixel format {:?}", image.pixel_format()),
                ));
                return;
            }
            if pixels.len() != 256 {
                report.push(TestCase::failed(
                    "codec.decode_avif_platform",
                    format!("expected 256 pixels bytes, got {}", pixels.len()),
                ));
                return;
            }

            let px = |x: usize, y: usize| {
                let off = (y * image.width() as usize + x) * 4;
                [
                    pixels[off],
                    pixels[off + 1],
                    pixels[off + 2],
                    pixels[off + 3],
                ]
            };
            let close =
                |a: [u8; 4], b: [u8; 4]| a.iter().zip(b.iter()).all(|(x, y)| x.abs_diff(*y) <= 8);
            let checks = [
                ((1, 1), [255, 0, 0, 255]),
                ((6, 1), [0, 255, 0, 255]),
                ((1, 6), [0, 0, 255, 255]),
                ((6, 6), [255, 255, 255, 255]),
            ];
            let bad = checks
                .iter()
                .filter(|((x, y), expected)| !close(px(*x, *y), *expected))
                .map(|((x, y), expected)| format!("({x},{y})={:?}!={:?}", px(*x, *y), expected))
                .collect::<Vec<_>>();
            if bad.is_empty() {
                report.push(TestCase::passed("codec.decode_avif_platform"));
            } else {
                report.push(TestCase::failed(
                    "codec.decode_avif_platform",
                    format!("quadrant pixels mismatch: {}", bad.join(" ")),
                ));
            }
        }
        Err(error) => report.push(TestCase::failed(
            "codec.decode_avif_platform",
            format!("decode_image failed: {error}"),
        )),
    }
}
