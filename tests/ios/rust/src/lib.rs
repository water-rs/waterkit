//! The Rust half of the iOS test harness: runs the enabled `WaterKit` cases
//! and returns their structured report to the Swift app.

use waterkit_test_report::{TestCase, TestReport, to_json_pretty};

#[cfg(feature = "camera")]
mod camera;

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
        camera::record(&mut report).await;

        #[cfg(feature = "clipboard")]
        record_clipboard(&mut report).await;

        #[cfg(feature = "fs")]
        record_fs(&mut report);

        #[cfg(feature = "haptic")]
        record_haptic(&mut report);

        #[cfg(feature = "notification")]
        report.push(TestCase::skipped(
            "notification.show",
            "notification authorization cannot be granted headless on the simulator",
        ));

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
        {
            let available = waterkit::nfc::is_available();
            if cfg!(target_abi = "sim") && available {
                report.push(TestCase::failed(
                    "nfc.availability",
                    "NFCNDEFReaderSession.readingAvailable is true on a simulator with no NFC hardware",
                ));
            } else {
                report.push(TestCase::passed_with_message(
                    "nfc.availability",
                    format!("available={available}"),
                ));
            }
        }

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
        report.push(TestCase::skipped(
            "health.availability",
            "waterkit-health declares extern Swift symbols but ships no Apple implementation",
        ));

        #[cfg(feature = "deeplink")]
        report.push(TestCase::passed("deeplink.linked"));

        #[cfg(feature = "vision")]
        record_vision(&mut report).await;
    });

    // Every enabled feature records at least one case, so an empty report
    // means the harness was built without any feature.
    if report.cases.is_empty() {
        report.push(TestCase::failed(
            "harness.feature",
            "no WaterKit feature was enabled for the iOS harness",
        ));
    }

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

#[cfg(feature = "clipboard")]
async fn record_clipboard(report: &mut TestReport) {
    let mut clipboard = match waterkit::clipboard::Clipboard::new() {
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
    report.push(TestCase::passed("clipboard.set_text"));

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

    report.push(match clipboard_files_round_trip(&mut clipboard).await {
        Ok(url) => {
            TestCase::passed_with_message("clipboard.files_round_trip", format!("url={url}"))
        }
        Err(error) => TestCase::failed("clipboard.files_round_trip", error),
    });
}

/// Copies two files, one with a name every URL encoding must escape, and
/// checks that the pasteboard carries the first file's contents, which other
/// apps paste, and its file URL, and that the paths read back unchanged.
#[cfg(feature = "clipboard")]
async fn clipboard_files_round_trip(
    clipboard: &mut waterkit::clipboard::Clipboard,
) -> Result<String, String> {
    const NAME: &str = "waterkit clipboard n\u{e4}me #1?.txt";
    const ENCODED_NAME: &str = "waterkit%20clipboard%20n%C3%A4me%20%231%3F.txt";
    const CONTENTS: &[u8] = b"WaterKit clipboard file\n";

    let dir = std::env::temp_dir();
    let paths = vec![dir.join(NAME), dir.join("plain.txt")];
    for path in &paths {
        std::fs::write(path, CONTENTS)
            .map_err(|error| format!("writing {}: {error}", path.display()))?;
    }
    clipboard
        .set_files(&paths)
        .map_err(|error| format!("set_files failed: {error}"))?;
    if !clipboard
        .has_files()
        .map_err(|error| format!("has_files failed: {error}"))?
    {
        return Err("has_files reported no files right after set_files".into());
    }
    let contents = clipboard
        .binary("public.plain-text")
        .await
        .map_err(|error| format!("reading public.plain-text failed: {error}"))?;
    if contents.as_deref() != Some(CONTENTS) {
        return Err(format!(
            "the pasteboard's public.plain-text is {contents:?}"
        ));
    }
    let url = clipboard
        .binary("public.file-url")
        .await
        .map_err(|error| format!("reading public.file-url failed: {error}"))?
        .ok_or("the pasteboard has no public.file-url")?;
    // The representation is a binary property list that holds the URL
    // string; take the URL from its scheme to the copied file's name.
    let url = String::from_utf8_lossy(&url);
    let url = url
        .find("file:///")
        .and_then(|start| {
            let end = start + url[start..].find(ENCODED_NAME)? + ENCODED_NAME.len();
            Some(url[start..end].to_owned())
        })
        .ok_or_else(|| format!("public.file-url holds no URL ending in {ENCODED_NAME}: {url:?}"))?;
    let read = clipboard
        .files()
        .await
        .map_err(|error| format!("files failed: {error}"))?;
    if read != paths {
        return Err(format!("url={url} files() returned {read:?} for {paths:?}"));
    }
    if url.contains(' ') {
        return Err(format!("the pasteboard's file URL is {url}"));
    }
    Ok(url)
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
        Err(waterkit::haptic::HapticError::Unsupported) => report.push(TestCase::skipped(
            "haptic.notification_success",
            "haptic engine unsupported on this device (simulator has no Taptic Engine)",
        )),
        Err(error) => report.push(TestCase::failed(
            "haptic.notification_success",
            format!("haptic feedback failed: {error}"),
        )),
    }
}

#[cfg(feature = "secret")]
async fn record_secret(report: &mut TestReport) {
    if let Err(error) =
        waterkit::secret::SecretManager::set("waterkit", "ios_test", "secret123").await
    {
        report.push(TestCase::failed(
            "secret.set",
            format!("secret set failed: {error}"),
        ));
        return;
    }
    report.push(TestCase::passed("secret.set"));

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
    match waterkit::system::connectivity() {
        Ok(connectivity) => report.push(TestCase::passed_with_message(
            "system.connectivity",
            format!("connection_type={:?}", connectivity.connection_type()),
        )),
        Err(error) => report.push(TestCase::failed(
            "system.connectivity",
            format!("connectivity query failed: {error}"),
        )),
    }
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

/// `VisionKit`'s `DataScannerViewController` reports unsupported on the
/// simulator, so `capabilities()` must say so and `scan()` must fail with
/// [`waterkit::vision::VisionError::Unsupported`] instead of presenting a
/// UI or falling back to another realization.
#[cfg(feature = "vision")]
async fn record_vision(report: &mut TestReport) {
    let available = waterkit::vision::CodeScanner::capabilities().available;
    if cfg!(target_abi = "sim") && available {
        report.push(TestCase::failed(
            "vision.scanner_capabilities",
            "DataScannerViewController reports supported on a simulator with no camera",
        ));
        return;
    }
    report.push(TestCase::passed_with_message(
        "vision.scanner_capabilities",
        format!("available={available}"),
    ));
    if available {
        report.push(TestCase::skipped(
            "vision.scanner_scan",
            "presenting the code scanner requires an interactive session",
        ));
        return;
    }
    match waterkit::vision::CodeScanner::new(waterkit::vision::Symbology::Qr)
        .scan()
        .await
    {
        Err(waterkit::vision::VisionError::Unsupported(message)) => {
            report.push(TestCase::passed_with_message(
                "vision.scanner_scan",
                format!("unsupported: {message}"),
            ));
        }
        other => report.push(TestCase::failed(
            "vision.scanner_scan",
            format!("scan() on an unsupported device returned {other:?}"),
        )),
    }
}

#[cfg(feature = "passkey")]
async fn record_passkey(report: &mut TestReport) {
    match waterkit::passkey::is_available().await {
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
            "passkey reports unsupported on iOS 16+",
        )),
        Err(error) => report.push(TestCase::failed(
            "passkey.availability",
            format!("passkey availability failed: {error}"),
        )),
    }
}
