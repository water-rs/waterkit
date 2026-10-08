//! The Rust half of the iOS test harness: runs the enabled `WaterKit` cases
//! and returns their structured report to the Swift app.

use waterkit_test_report::{TestCase, TestReport, to_json_pretty};

#[cfg(feature = "camera")]
mod camera;
#[cfg(feature = "vision")]
mod vision;

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

#[allow(clippy::too_many_lines)]
fn build_report() -> TestReport {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime for iOS test harness");
    let mut report = Harness {
        runtime: &runtime,
        report: TestReport::new("ios", "waterkit-test-ios"),
    }
    .run();

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

/// What every capability's recorder shares: the runtime its asynchronous
/// calls run on, and the report its cases go into.
struct Harness<'h> {
    runtime: &'h tokio::runtime::Runtime,
    report: TestReport,
}

impl Harness<'_> {
    /// Runs every enabled capability's recorder and returns the report.
    fn run(mut self) -> TestReport {
        // Every recorder runs in the runtime's context, and the asynchronous
        // ones drive their future on it.
        let _runtime_context = self.runtime.enter();
        for record in RECORDERS {
            record(&mut self);
        }
        self.report
    }
}

/// Records one capability's cases.
type Recorder = fn(&mut Harness<'_>);

/// The recorder of every enabled capability, in report order.
const RECORDERS: &[Recorder] = &[
    #[cfg(feature = "sensor")]
    |h| h.runtime.block_on(record_sensor(&mut h.report)),
    #[cfg(feature = "location")]
    |h| h.runtime.block_on(record_location(&mut h.report)),
    #[cfg(feature = "permission")]
    |h| h.runtime.block_on(record_permission(&mut h.report)),
    #[cfg(feature = "camera")]
    |h| h.runtime.block_on(camera::record(&mut h.report)),
    #[cfg(feature = "clipboard")]
    |h| h.runtime.block_on(record_clipboard(&mut h.report)),
    #[cfg(feature = "fs")]
    |h| record_fs(&mut h.report),
    #[cfg(feature = "haptic")]
    |h| record_haptic(&mut h.report),
    #[cfg(feature = "notification")]
    |h| {
        h.report.push(TestCase::skipped(
            "notification.show",
            "notification authorization cannot be granted headless on the simulator",
        ));
    },
    #[cfg(feature = "secret")]
    |h| h.runtime.block_on(record_secret(&mut h.report)),
    #[cfg(feature = "system")]
    |h| record_system(&mut h.report),
    #[cfg(feature = "screen")]
    |h| record_screen(&mut h.report),
    #[cfg(feature = "background")]
    |h| record_background(&mut h.report),
    #[cfg(feature = "passkey")]
    |h| h.runtime.block_on(record_passkey(&mut h.report)),
    #[cfg(feature = "language")]
    |h| h.runtime.block_on(record_language(&mut h.report)),
    #[cfg(feature = "wallet")]
    |h| h.runtime.block_on(record_wallet(&mut h.report)),
    #[cfg(feature = "biometric")]
    |h| {
        h.report.push(TestCase::skipped(
            "biometric.authenticate",
            "biometric authentication requires an interactive prompt",
        ));
    },
    #[cfg(feature = "audio")]
    |h| h.report.push(TestCase::passed("audio.linked")),
    #[cfg(feature = "codec")]
    |h| h.report.push(TestCase::passed("codec.linked")),
    #[cfg(feature = "dialog")]
    |h| h.report.push(TestCase::passed("dialog.linked")),
    #[cfg(feature = "video")]
    |h| h.report.push(TestCase::passed("video.linked")),
    #[cfg(feature = "bluetooth")]
    |h| h.report.push(TestCase::passed("bluetooth.linked")),
    #[cfg(feature = "nfc")]
    |h| record_nfc(&mut h.report),
    #[cfg(feature = "share")]
    |h| {
        h.report.push(TestCase::skipped(
            "share.sheet",
            "share sheet requires an interactive chooser",
        ));
    },
    #[cfg(feature = "speech")]
    |h| {
        h.report.push(TestCase::skipped(
            "speech.tts",
            "speech synthesis is audible and not asserted by this harness",
        ));
    },
    #[cfg(feature = "contacts")]
    |h| {
        h.report.push(TestCase::skipped(
            "contacts.fetch_all",
            "contacts access depends on runtime user data permissions",
        ));
    },
    #[cfg(feature = "calendar")]
    |h| {
        h.report.push(TestCase::skipped(
            "calendar.list",
            "calendar access depends on runtime user data permissions",
        ));
    },
    #[cfg(feature = "health")]
    |h| {
        h.report.push(TestCase::skipped(
            "health.availability",
            "waterkit-health declares extern Swift symbols but ships no Apple implementation",
        ));
    },
    #[cfg(feature = "deeplink")]
    |h| h.report.push(TestCase::passed("deeplink.linked")),
    #[cfg(feature = "vision")]
    |h| h.runtime.block_on(record_vision(&mut h.report)),
    #[cfg(feature = "vision")]
    |h| h.runtime.block_on(vision::record(&mut h.report)),
];

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

#[cfg(feature = "nfc")]
fn record_nfc(report: &mut TestReport) {
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

/// `VisionKit`'s `DataScannerViewController` and
/// `VNDocumentCameraViewController` report unsupported on the simulator, so
/// `capabilities()` must say so and `scan()` must fail with
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
    } else {
        report.push(TestCase::passed_with_message(
            "vision.scanner_capabilities",
            format!("available={available}"),
        ));
        if available {
            report.push(TestCase::skipped(
                "vision.scanner_scan",
                "presenting the code scanner requires an interactive session",
            ));
        } else {
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
    }

    // `VNDocumentCameraViewController.isSupported` reports false where no
    // camera can scan, like the simulator.
    let document_available = waterkit::vision::DocumentScanner::capabilities().available;
    if cfg!(target_abi = "sim") && document_available {
        report.push(TestCase::failed(
            "vision.document_scanner_capabilities",
            "VNDocumentCameraViewController reports supported on a simulator with no camera",
        ));
    } else {
        report.push(TestCase::passed_with_message(
            "vision.document_scanner_capabilities",
            format!("available={document_available}"),
        ));
        if document_available {
            report.push(TestCase::skipped(
                "vision.document_scanner_scan",
                "presenting the document scanner requires an interactive session",
            ));
        } else {
            match waterkit::vision::DocumentScanner::new().scan().await {
                Err(waterkit::vision::VisionError::Unsupported(message)) => {
                    report.push(TestCase::passed_with_message(
                        "vision.document_scanner_scan",
                        format!("unsupported: {message}"),
                    ));
                }
                other => report.push(TestCase::failed(
                    "vision.document_scanner_scan",
                    format!("scan() on an unsupported device returned {other:?}"),
                )),
            }
        }
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

#[cfg(feature = "language")]
async fn record_language(report: &mut TestReport) {
    use waterkit::language::translation;

    let capabilities = match translation::capabilities().await {
        Ok(capabilities) => capabilities,
        Err(error) => {
            record_language_query_failure(report, &error);
            return;
        }
    };
    record_language_capabilities(report, &capabilities);
    record_language_translation(report, &capabilities).await;
    record_language_not_installed(report, &capabilities).await;
}

#[cfg(feature = "language")]
fn record_language_query_failure(
    report: &mut TestReport,
    error: &waterkit::language::translation::TranslationError,
) {
    report.push(TestCase::failed(
        "language.capabilities",
        format!("capability query failed: {error}"),
    ));
    report.push(TestCase::skipped(
        "language.translate",
        "capability query failed",
    ));
    report.push(TestCase::skipped(
        "language.not_installed",
        "capability query failed",
    ));
}

#[cfg(feature = "language")]
fn record_language_capabilities(
    report: &mut TestReport,
    capabilities: &waterkit::language::translation::TranslationCapabilities,
) {
    let pairs = capabilities
        .pairs()
        .iter()
        .map(|capability| format!("{}={:?}", capability.pair(), capability.status()))
        .collect::<Vec<_>>();
    report.push(TestCase::passed_with_message(
        "language.capabilities",
        if pairs.is_empty() {
            "0 pairs (no on-device translation pairs on this OS)".into()
        } else {
            pairs.join(", ")
        },
    ));
}

#[cfg(feature = "language")]
async fn record_language_translation(
    report: &mut TestReport,
    capabilities: &waterkit::language::translation::TranslationCapabilities,
) {
    use waterkit::language::translation::{AssetStatus, Translator};

    if let Some(capability) = capabilities
        .pairs()
        .iter()
        .find(|capability| capability.status() == AssetStatus::Installed)
    {
        let pair = capability.pair().clone();
        match tokio::time::timeout(
            std::time::Duration::from_secs(20),
            Translator::new(pair.source().clone(), pair.target().clone()),
        )
        .await
        {
            Err(_) => report.push(TestCase::failed(
                "language.translate",
                "system translation service did not create a translator within 20 s",
            )),
            Ok(Ok(translator)) => match tokio::time::timeout(
                std::time::Duration::from_secs(20),
                translator.translate_batch(&["Hello", "Good morning"]),
            )
            .await
            {
                Err(_) => report.push(TestCase::failed(
                    "language.translate",
                    "system translation service did not translate within 20 s",
                )),
                Ok(Ok(translations))
                    if translations.len() == 2
                        && translations
                            .iter()
                            .all(|translation| !translation.trim().is_empty()) =>
                {
                    report.push(TestCase::passed_with_message(
                        "language.translate",
                        format!("{} => {translations:?}", translator.pair()),
                    ));
                }
                Ok(Ok(translations)) => report.push(TestCase::failed(
                    "language.translate",
                    format!("invalid batch result: {translations:?}"),
                )),
                Ok(Err(error)) => report.push(TestCase::failed(
                    "language.translate",
                    format!("batch translation failed: {error}"),
                )),
            },
            Ok(Err(error)) => report.push(TestCase::failed(
                "language.translate",
                format!("translator creation failed: {error}"),
            )),
        }
    } else {
        report.push(TestCase::skipped("language.translate", "no installed pair"));
    }
}

#[cfg(feature = "language")]
async fn record_language_not_installed(
    report: &mut TestReport,
    capabilities: &waterkit::language::translation::TranslationCapabilities,
) {
    use waterkit::language::translation::{AssetStatus, TranslationError, Translator};

    if let Some(capability) = capabilities
        .pairs()
        .iter()
        .find(|capability| capability.status() == AssetStatus::NeedsDownload)
    {
        let pair = capability.pair().clone();
        match Translator::new(pair.source().clone(), pair.target().clone()).await {
            Err(TranslationError::NeedsDownload(actual)) if actual == pair => {
                report.push(TestCase::passed_with_message(
                    "language.not_installed",
                    format!("{actual} needs download"),
                ));
            }
            Err(error) => report.push(TestCase::failed(
                "language.not_installed",
                format!("expected NeedsDownload for {pair}, got {error}"),
            )),
            Ok(_) => report.push(TestCase::failed(
                "language.not_installed",
                format!("unexpectedly created a translator for {pair}"),
            )),
        }
    } else {
        report.push(TestCase::skipped(
            "language.not_installed",
            "no pair needs downloading",
        ));
    }
}

#[cfg(feature = "wallet")]
async fn record_wallet(report: &mut TestReport) {
    match waterkit::wallet::capabilities().await {
        Ok(capabilities) => report.push(TestCase::passed_with_message(
            "wallet.availability",
            format!("available={}", capabilities.available),
        )),
        Err(error) => report.push(TestCase::failed(
            "wallet.availability",
            format!("wallet capability probe failed: {error}"),
        )),
    }
}
