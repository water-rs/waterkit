use waterkit_test_report::{TestCase, TestReport, to_json_pretty};

#[swift_bridge::bridge]
mod ffi {
    extern "Rust" {
        fn run_tests();
        fn run_tests_json() -> String;
    }
}

fn run_tests() {
    let _ = run_tests_json();
}

fn run_tests_json() -> String {
    let report = build_report();
    to_json_pretty(&report).expect("failed to serialize WaterKit iOS test report")
}

fn build_report() -> TestReport {
    let mut report = TestReport::new("ios", "waterkit-test-ios");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime for iOS test harness");

    rt.block_on(async {
        #[cfg(feature = "sensor")]
        record_sensor(&mut report).await;

        #[cfg(feature = "location")]
        record_location(&mut report).await;

        #[cfg(feature = "permission")]
        record_permission(&mut report).await;

        #[cfg(feature = "camera")]
        record_camera(&mut report);

        #[cfg(feature = "clipboard")]
        record_clipboard(&mut report);

        #[cfg(feature = "fs")]
        record_fs(&mut report);

        #[cfg(feature = "haptic")]
        record_haptic(&mut report);

        #[cfg(feature = "notification")]
        record_notification(&mut report);

        #[cfg(feature = "secret")]
        record_secret(&mut report).await;

        #[cfg(feature = "system")]
        record_system(&mut report);

        #[cfg(feature = "screen")]
        record_screen(&mut report);

        #[cfg(feature = "background")]
        record_background(&mut report);

        #[cfg(feature = "passkey")]
        record_passkey(&mut report).await;

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
        report.push(TestCase::passed_with_message(
            "nfc.availability",
            format!("available={}", waterkit::nfc::is_available()),
        ));

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
            format!("available={}", waterkit::health::capabilities().available),
        ));

        #[cfg(feature = "deeplink")]
        report.push(TestCase::passed("deeplink.linked"));

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
            feature = "screen",
            feature = "bluetooth",
            feature = "nfc",
            feature = "share",
            feature = "speech",
            feature = "contacts",
            feature = "calendar",
            feature = "health",
            feature = "deeplink",
            feature = "background",
            feature = "passkey"
        )))]
        report.push(TestCase::failed(
            "harness.feature",
            "no WaterKit feature was enabled for the iOS harness",
        ));
    });

    report
}

#[cfg(feature = "sensor")]
async fn record_sensor(report: &mut TestReport) {
    if !waterkit::sensor::Accelerometer::capabilities().available {
        report.push(TestCase::skipped(
            "sensor.accelerometer",
            "accelerometer is unavailable on this device",
        ));
        return;
    }

    match waterkit::sensor::Accelerometer::read().await {
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
        Err(error) => report.push(TestCase::failed(
            "sensor.accelerometer",
            format!("accelerometer reported available but read failed: {error}"),
        )),
    }
}

#[cfg(feature = "location")]
async fn record_location(report: &mut TestReport) {
    match waterkit::permission::check(waterkit::permission::Permission::Location).await {
        waterkit::permission::PermissionStatus::Granted => {}
        status => {
            report.push(TestCase::skipped(
                "location.get",
                format!("location permission is {status:?}"),
            ));
            return;
        }
    }

    match waterkit::location::Location::get().await {
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
        Err(error) => report.push(TestCase::failed(
            "location.get",
            format!("location read failed: {error}"),
        )),
    }
}

#[cfg(feature = "permission")]
async fn record_permission(report: &mut TestReport) {
    let status = waterkit::permission::check(waterkit::permission::Permission::Location).await;
    report.push(TestCase::passed_with_message(
        "permission.location",
        format!("status={status:?}"),
    ));
}

#[cfg(feature = "camera")]
fn record_camera(report: &mut TestReport) {
    match waterkit::camera::Camera::list() {
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
fn record_clipboard(report: &mut TestReport) {
    match waterkit::clipboard::Clipboard::new() {
        Ok(mut clipboard) => match clipboard.set_text("WaterKit Test") {
            Ok(()) => report.push(TestCase::passed("clipboard.set_text")),
            Err(error) => report.push(TestCase::failed(
                "clipboard.set_text",
                format!("set_text failed: {error}"),
            )),
        },
        Err(error) => report.push(TestCase::failed(
            "clipboard.init",
            format!("clipboard init failed: {error}"),
        )),
    }
}

#[cfg(feature = "fs")]
fn record_fs(report: &mut TestReport) {
    match waterkit::fs::WaterFs::cache_dir() {
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
fn record_haptic(report: &mut TestReport) {
    match waterkit::haptic::Haptic::notification_success() {
        Ok(()) => report.push(TestCase::passed("haptic.notification_success")),
        Err(error) => report.push(TestCase::failed(
            "haptic.notification_success",
            format!("haptic feedback failed: {error}"),
        )),
    }
}

#[cfg(feature = "notification")]
fn record_notification(report: &mut TestReport) {
    match waterkit::notification::Notification::new()
        .title("WaterKit Test")
        .body("iOS notification is working")
        .show()
    {
        Ok(_handle) => report.push(TestCase::passed("notification.show")),
        Err(error) => report.push(TestCase::failed(
            "notification.show",
            format!("notification show failed: {error}"),
        )),
    }
}

#[cfg(feature = "secret")]
async fn record_secret(report: &mut TestReport) {
    match waterkit::secret::SecretManager::set("waterkit", "ios_test", "secret123").await {
        Ok(()) => {}
        Err(error) => {
            report.push(TestCase::failed(
                "secret.set",
                format!("secret set failed: {error}"),
            ));
            return;
        }
    }

    match waterkit::secret::SecretManager::get("waterkit", "ios_test").await {
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

    match waterkit::secret::SecretManager::delete("waterkit", "ios_test").await {
        Ok(()) => report.push(TestCase::passed("secret.delete")),
        Err(error) => report.push(TestCase::failed(
            "secret.delete",
            format!("secret delete failed: {error}"),
        )),
    }
}

#[cfg(feature = "system")]
fn record_system(report: &mut TestReport) {
    let connectivity = waterkit::system::connectivity();
    report.push(TestCase::passed_with_message(
        "system.connectivity",
        format!("connection_type={:?}", connectivity.connection_type()),
    ));
}

#[cfg(feature = "screen")]
fn record_screen(report: &mut TestReport) {
    match waterkit::screen::screens() {
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

#[cfg(feature = "background")]
fn record_background(report: &mut TestReport) {
    let capabilities = waterkit::background::capabilities();
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
async fn record_passkey(report: &mut TestReport) {
    match waterkit::passkey::is_available().await {
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
