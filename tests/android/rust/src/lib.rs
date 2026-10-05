//! Android JNI generic test harness.

#![cfg(target_os = "android")]

use jni::errors::ThrowRuntimeExAndDefault;
#[cfg(feature = "location")]
use jni::objects::JDoubleArray;
use jni::objects::{Global, JObject};
use jni::sys::{jdoubleArray, jstring};
use jni::{Env, EnvUnowned};
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
    fn new(env: &mut Env<'_>, activity: &JObject<'_>) -> jni::errors::Result<Self> {
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
}

fn run_native_report(_env: &mut Env<'_>, _activity: &JObject<'_>) -> TestReport {
    let mut report = TestReport::new("android", "waterkit-test-android");
    #[cfg(any(
        feature = "sensor",
        feature = "location",
        feature = "permission",
        feature = "fs",
        feature = "secret"
    ))]
    let activity_global = match _env.new_global_ref(_activity) {
        Ok(value) => value,
        Err(error) => {
            report.push(TestCase::failed(
                "harness.activity_ref",
                format!("failed to create global activity ref: {error}"),
            ));
            return report;
        }
    };
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
            feature = "secret"
        ))]
        let activity = activity_global.as_obj();

        #[cfg(feature = "sensor")]
        record_android_sensor(&mut report, _env, activity);

        #[cfg(feature = "location")]
        record_android_location(&mut report, _env, activity);

        #[cfg(feature = "permission")]
        record_android_permission(&mut report, _env, activity);

        #[cfg(feature = "camera")]
        record_android_camera(&mut report).await;

        #[cfg(feature = "clipboard")]
        record_android_clipboard(&mut report).await;

        #[cfg(feature = "fs")]
        record_android_fs(&mut report, _env, activity);

        #[cfg(feature = "haptic")]
        record_android_haptic(&mut report);

        #[cfg(feature = "notification")]
        record_android_notification(&mut report);

        #[cfg(feature = "secret")]
        record_android_secret(&mut report, _env, activity);

        #[cfg(feature = "system")]
        record_android_system(&mut report);

        #[cfg(feature = "background")]
        record_android_background(&mut report);

        #[cfg(feature = "passkey")]
        record_android_passkey(&mut report).await;

        #[cfg(feature = "biometric")]
        report.push(TestCase::skipped(
            "biometric.authenticate",
            "biometric authentication requires an interactive prompt",
        ));

        #[cfg(feature = "audio")]
        report.push(TestCase::passed("audio.linked"));

        #[cfg(feature = "codec")]
        {
            report.push(TestCase::passed("codec.linked"));
            record_android_avif_decode(&mut report);
        }

        #[cfg(feature = "dialog")]
        report.push(TestCase::passed("dialog.linked"));

        #[cfg(feature = "video")]
        report.push(TestCase::passed("video.linked"));

        #[cfg(feature = "bluetooth")]
        report.push(TestCase::passed("bluetooth.linked"));

        #[cfg(feature = "nfc")]
        report.push(TestCase::passed("nfc.linked"));

        #[cfg(feature = "share")]
        report.push(TestCase::skipped(
            "share.sheet",
            "share sheet requires an interactive chooser",
        ));

        #[cfg(feature = "speech")]
        report.push(TestCase::skipped(
            "speech.tts",
            "speech synthesis is audible and not asserted by this harness",
        ));

        #[cfg(feature = "contacts")]
        report.push(TestCase::skipped(
            "contacts.fetch_all",
            "contacts access depends on runtime user data permissions",
        ));

        #[cfg(feature = "calendar")]
        report.push(TestCase::skipped(
            "calendar.list",
            "calendar access depends on runtime user data permissions",
        ));

        #[cfg(feature = "health")]
        report.push(TestCase::passed_with_message(
            "health.availability",
            format!(
                "available={}",
                waterkit_content::health::capabilities().available
            ),
        ));

        #[cfg(feature = "deeplink")]
        report.push(TestCase::passed("deeplink.linked"));

        #[cfg(feature = "screen")]
        record_android_screen(&mut report);

        #[cfg(not(any(
            feature = "sensor",
            feature = "biometric",
            feature = "location",
            feature = "audio",
            feature = "camera",
            feature = "clipboard",
            feature = "codec",
            feature = "dialog",
            feature = "fs",
            feature = "haptic",
            feature = "notification",
            feature = "permission",
            feature = "secret",
            feature = "system",
            feature = "video",
            feature = "bluetooth",
            feature = "nfc",
            feature = "share",
            feature = "speech",
            feature = "deeplink",
            feature = "contacts",
            feature = "calendar",
            feature = "health",
            feature = "screen",
            feature = "background",
            feature = "passkey"
        )))]
        report.push(TestCase::failed(
            "harness.feature",
            "no WaterKit feature was enabled for the Android harness",
        ));
    });

    report
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

#[cfg(feature = "camera")]
async fn record_android_camera(report: &mut TestReport) {
    match waterkit_content::camera::Camera::list() {
        Ok(cameras) => {
            report.push(TestCase::passed_with_message(
                "camera.list",
                format!("count={}", cameras.len()),
            ));
            for camera in cameras {
                record_android_camera_frames(report, &camera).await;
            }
        }
        Err(error) => report.push(TestCase::failed(
            "camera.list",
            format!("camera list failed: {error}"),
        )),
    }
}

/// Streams a few frames from `camera`, converts the last one upright on the
/// GPU, and reports the frames' plane layout, orientation and sizes.
#[cfg(feature = "camera")]
async fn record_android_camera_frames(
    report: &mut TestReport,
    camera: &waterkit_content::camera::CameraInfo,
) {
    use futures::StreamExt;
    use std::sync::Arc;
    use waterkit_content::camera::{Camera, CameraConfig, FrameConverter, FramePlanes, wgpu};

    const FRAMES: usize = 5;
    let case = format!("camera.frames.{}", camera.id);

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = match instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
    {
        Ok(adapter) => adapter,
        Err(error) => {
            report.push(TestCase::failed(case, format!("no GPU adapter: {error}")));
            return;
        }
    };
    let (device, queue) = match adapter
        .request_device(&wgpu::DeviceDescriptor {
            required_features: FrameConverter::required_features(adapter.features()),
            ..Default::default()
        })
        .await
    {
        Ok(pair) => pair,
        Err(error) => {
            report.push(TestCase::failed(case, format!("no GPU device: {error}")));
            return;
        }
    };
    let device = Arc::new(device);
    let queue = Arc::new(queue);

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

    let frames = camera_handle.frames().take(FRAMES).collect::<Vec<_>>();
    let frames = match tokio::time::timeout(std::time::Duration::from_secs(15), frames).await {
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
                format!("no {FRAMES} frames within 15 s"),
            ));
            return;
        }
    };

    let orientations: Vec<_> = frames.iter().map(|frame| frame.orientation()).collect();
    let last = frames.last().expect("FRAMES frames were collected");
    let layout = match last.planes() {
        FramePlanes::Rgb(_) => "rgb",
        FramePlanes::YCbCr420 { .. } => "ycbcr420",
        FramePlanes::YCbCr422 { .. } => "ycbcr422",
    };
    let upright = FrameConverter::new(&device).convert(&device, &queue, last);
    report.push(TestCase::passed_with_message(
        case,
        format!(
            "front={} layout={layout} stored={}x{} orientations={orientations:?} upright={}x{}",
            camera.is_front_facing,
            last.width(),
            last.height(),
            upright.width(),
            upright.height(),
        ),
    ));
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
    const AFTER_LIFECYCLE: &str = "WaterKit Watch After Lifecycle";

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

    match (primary_first, primary_second) {
        (ClipWait::Event(first), ClipWait::Event(second))
            if first.has_text() && second.has_text() =>
        {
            report.push(TestCase::passed("clipboard.watch_same_type"));
        }
        (first, second) => report.push(TestCase::failed(
            "clipboard.watch_same_type",
            format!("expected two text events, got {first:?} then {second:?}"),
        )),
    }

    match (second_first, second_second) {
        (ClipWait::Event(first), ClipWait::Event(second))
            if first.has_text() && second.has_text() =>
        {
            report.push(TestCase::passed("clipboard.watch_independent_subscribers"));
        }
        (first, second) => report.push(TestCase::failed(
            "clipboard.watch_independent_subscribers",
            format!("expected two text events, got {first:?} then {second:?}"),
        )),
    }

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
    if let ClipWait::Closed = wait {
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
    let connectivity = waterkit_content::system::connectivity();
    let thermal = waterkit_content::system::thermal_state();
    report.push(TestCase::passed_with_message(
        "system.snapshot",
        format!(
            "connectivity={:?} thermal={thermal:?}",
            connectivity.connection_type()
        ),
    ));
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
            ))
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

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_waterkit_test_MainActivity_testCheckPermission<'local>(
    mut unowned_env: EnvUnowned<'local>,
    _this: JObject<'local>,
    activity: JObject<'local>,
    _permission_type: i32,
) -> i32 {
    unowned_env
        .with_env(|env| -> jni::errors::Result<i32> {
            Ok(check_permission(env, &activity, _permission_type))
        })
        .resolve::<ThrowRuntimeExAndDefault>()
}

fn check_permission(_env: &mut Env<'_>, _activity: &JObject<'_>, _permission_type: i32) -> i32 {
    #[cfg(feature = "permission")]
    {
        let permission = match _permission_type {
            0 => waterkit_content::permission::Permission::Location,
            1 => waterkit_content::permission::Permission::Camera,
            2 => waterkit_content::permission::Permission::Microphone,
            3 => waterkit_content::permission::Permission::Photos,
            4 => waterkit_content::permission::Permission::Contacts,
            5 => waterkit_content::permission::Permission::Calendar,
            _ => {
                log::error!("Unknown permission type: {_permission_type}");
                return PERMISSION_NOT_DETERMINED;
            }
        };

        match waterkit_content::permission::android::check_with_activity(
            _env, _activity, permission,
        ) {
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
        let _ = (_env, _activity);
        log::error!("testCheckPermission called without enabling permission feature");
        PERMISSION_NOT_DETERMINED
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_waterkit_test_MainActivity_testGetLocation<'local>(
    mut unowned_env: EnvUnowned<'local>,
    _this: JObject<'local>,
    activity: JObject<'local>,
) -> jdoubleArray {
    unowned_env
        .with_env(|env| -> jni::errors::Result<jdoubleArray> { Ok(get_location(env, &activity)) })
        .resolve::<ThrowRuntimeExAndDefault>()
}

fn get_location(_env: &mut Env<'_>, _activity: &JObject<'_>) -> jdoubleArray {
    #[cfg(feature = "location")]
    {
        match waterkit_content::location::android::get_location_with_context(_env, _activity) {
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

                let array = match JDoubleArray::new(_env, payload.len()) {
                    Ok(arr) => arr,
                    Err(error) => {
                        log::error!("JDoubleArray::new failed: {error}");
                        return std::ptr::null_mut();
                    }
                };

                if let Err(error) = array.set_region(_env, 0, &payload) {
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
        let _ = (_env, _activity);
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
            if !bad.is_empty() {
                report.push(TestCase::failed(
                    "codec.decode_avif_platform",
                    format!("quadrant pixels mismatch: {}", bad.join(" ")),
                ));
            } else {
                report.push(TestCase::passed("codec.decode_avif_platform"));
            }
        }
        Err(error) => report.push(TestCase::failed(
            "codec.decode_avif_platform",
            format!("decode_image failed: {error}"),
        )),
    }
}
