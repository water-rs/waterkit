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
        record_android_camera(&mut report);

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
        report.push(TestCase::passed("codec.linked"));

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
fn record_android_camera(report: &mut TestReport) {
    match waterkit_content::camera::Camera::list() {
        Ok(cameras) => report.push(TestCase::passed_with_message(
            "camera.list",
            format!("count={}", cameras.len()),
        )),
        Err(error) => report.push(TestCase::failed(
            "camera.list",
            format!("camera list failed: {error}"),
        )),
    }
}

#[cfg(feature = "clipboard")]
async fn record_android_clipboard(report: &mut TestReport) {
    match waterkit_content::clipboard::Clipboard::new() {
        Ok(mut clipboard) => {
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
                Ok(text) => report.push(TestCase::failed(
                    "clipboard.round_trip",
                    format!("expected WaterKit Test, got {text:?}"),
                )),
                Err(error) => report.push(TestCase::failed(
                    "clipboard.round_trip",
                    format!("get_text failed: {error}"),
                )),
            }
        }
        Err(error) => report.push(TestCase::failed(
            "clipboard.init",
            format!("clipboard init failed: {error}"),
        )),
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
            report.push(TestCase::failed(
                "secret.set",
                format!("secret set failed: {error}"),
            ));
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
        Ok(availability) => report.push(TestCase::passed_with_message(
            "passkey.availability",
            format!(
                "supported={} user_verification={} discoverable={}",
                availability.is_platform_supported,
                availability.supports_user_verification,
                availability.supports_discoverable_credentials
            ),
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
