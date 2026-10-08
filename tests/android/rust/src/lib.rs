//! Android JNI generic test harness.

#![cfg(target_os = "android")]

#[cfg(feature = "otp")]
use jni::JavaVM;
use jni::errors::ThrowRuntimeExAndDefault;
#[cfg(feature = "location")]
use jni::objects::JDoubleArray;
#[cfg(any(feature = "clipboard", feature = "dialog", feature = "otp"))]
use jni::objects::JValue;
#[cfg(feature = "otp")]
use jni::objects::JValueOwned;
use jni::objects::{Global, JObject};
use jni::sys::{jboolean, jdoubleArray, jstring};
use jni::{Env, EnvUnowned};
#[cfg(feature = "otp")]
use jni::{jni_sig, jni_str};
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "otp")]
use waterkit_build::decode_string;
#[cfg(any(feature = "clipboard", feature = "otp"))]
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
        let _android_context = match AndroidContextOwner::new(env, &activity) {
            Ok(owner) => owner,
            Err(AndroidContextOwnerError::AlreadyActive) => {
                log::error!(
                    "Android test context is already active; do not run a manual OTP request and runTest concurrently"
                );
                return Ok(());
            }
            Err(AndroidContextOwnerError::Jni(error)) => return Err(error),
        };
        let report = run_native_report(env, &activity, false, false);
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
    sms_delivery: jboolean,
    interactive: jboolean,
) -> jstring {
    init_logger();
    env.with_env(|env| -> jni::errors::Result<jstring> {
        let _android_context = match AndroidContextOwner::new(env, &activity) {
            Ok(owner) => owner,
            Err(AndroidContextOwnerError::AlreadyActive) => {
                let mut report = TestReport::new("android", "waterkit-test-android");
                report.push(TestCase::failed(
                    "harness.android_context",
                    "Android context is already active; do not run a manual OTP request and runTestReport concurrently",
                ));
                log_report(&report);
                return match env.new_string(to_json_pretty(&report).map_err(|error| {
                    jni::errors::Error::ParseFailed(error.to_string())
                })?) {
                    Ok(value) => Ok(value.into_raw()),
                    Err(error) => Err(error),
                };
            }
            Err(AndroidContextOwnerError::Jni(error)) => return Err(error),
        };
        let report = run_native_report(env, &activity, sms_delivery, interactive);
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

#[cfg(feature = "otp")]
#[derive(Clone, Copy)]
enum ManualOtpMode {
    Addressed,
    Consent,
}

#[cfg(feature = "otp")]
/// Starts a manual addressed OTP request from the Android UI.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_waterkit_test_MainActivity_testOtpAddressed<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    activity: JObject<'local>,
) {
    init_logger();
    env.with_env(|env| start_manual_otp(env, &activity, ManualOtpMode::Addressed))
        .resolve::<ThrowRuntimeExAndDefault>();
}

#[cfg(feature = "otp")]
/// Starts a manual SMS User Consent request from the Android UI.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_waterkit_test_MainActivity_testOtpConsent<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    activity: JObject<'local>,
) {
    init_logger();
    env.with_env(|env| start_manual_otp(env, &activity, ManualOtpMode::Consent))
        .resolve::<ThrowRuntimeExAndDefault>();
}

#[cfg(feature = "otp")]
fn start_manual_otp(
    env: &mut Env<'_>,
    activity: &JObject<'_>,
    mode: ManualOtpMode,
) -> jni::errors::Result<()> {
    let owner = match AndroidContextOwner::new(env, activity) {
        Ok(owner) => owner,
        Err(AndroidContextOwnerError::AlreadyActive) => {
            let message = "waterkit-otp error=Android context is already active; do not run manual OTP and runTestReport concurrently";
            log::error!(target: "waterkit-otp", "{message}");
            activity_log(env, activity, message)?;
            activity_finish_otp(env, activity)?;
            return Ok(());
        }
        Err(AndroidContextOwnerError::Jni(error)) => return Err(error),
    };
    let java_vm = env.get_java_vm()?;
    let activity_for_worker = env.new_global_ref(activity)?;
    let spawn_result = std::thread::Builder::new()
        .name("waterkit-otp-manual".to_owned())
        .spawn(move || {
            run_manual_otp(&java_vm, &activity_for_worker, mode);
            drop(owner);
        });
    if let Err(error) = spawn_result {
        let message = format!("waterkit-otp error=failed to start request thread: {error}");
        log::error!(target: "waterkit-otp", "{message}");
        activity_log(env, activity, &message)?;
        activity_finish_otp(env, activity)?;
    }
    Ok(())
}

#[cfg(feature = "otp")]
fn run_manual_otp(java_vm: &JavaVM, activity: &Global<JObject<'static>>, mode: ManualOtpMode) {
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())
        .and_then(|runtime| runtime.block_on(run_manual_otp_request(java_vm, activity, mode)));
    if let Err(error) = result {
        let message = format!("waterkit-otp error={error}");
        log::error!(target: "waterkit-otp", "{message}");
        activity_log_attached(java_vm, activity, &message);
    }
    if let Err(error) = activity_finish_otp_attached(java_vm, activity) {
        log::warn!(target: "waterkit-otp", "failed to finish manual OTP UI state: {error}");
    }
}

#[cfg(feature = "otp")]
async fn run_manual_otp_request(
    java_vm: &JavaVM,
    activity: &Global<JObject<'static>>,
    mode: ManualOtpMode,
) -> Result<(), String> {
    use waterkit_content::otp::{AddressedRequest, ConsentRequest};

    match mode {
        ManualOtpMode::Addressed => {
            let request = AddressedRequest::start()
                .await
                .map_err(|error| error.to_string())?;
            let token_line = format!("waterkit-otp token={}", request.token());
            log::info!(target: "waterkit-otp", "{token_line}");
            activity_log_attached(java_vm, activity, &token_line);
            let message = request.message().await.map_err(|error| error.to_string())?;
            let message_line = format!("waterkit-otp message={message}");
            log::info!(target: "waterkit-otp", "{message_line}");
            activity_log_attached(java_vm, activity, &message_line);
        }
        ManualOtpMode::Consent => {
            let request = ConsentRequest::start(None)
                .await
                .map_err(|error| error.to_string())?;
            let message = request.message().await.map_err(|error| error.to_string())?;
            let message_line = format!("waterkit-otp message={message}");
            log::info!(target: "waterkit-otp", "{message_line}");
            activity_log_attached(java_vm, activity, &message_line);
        }
    }
    Ok(())
}

#[cfg(feature = "otp")]
fn activity_log(
    env: &mut Env<'_>,
    activity: &JObject<'_>,
    message: &str,
) -> jni::errors::Result<()> {
    let message = env.new_string(message)?;
    env.call_method(
        activity,
        jni_str!("logFromNative"),
        jni_sig!("(Ljava/lang/String;)V"),
        &[JValue::Object(&message)],
    )?;
    Ok(())
}

#[cfg(feature = "otp")]
fn activity_finish_otp(env: &mut Env<'_>, activity: &JObject<'_>) -> jni::errors::Result<()> {
    env.call_method(activity, jni_str!("finishOtpRequest"), jni_sig!("()V"), &[])?;
    Ok(())
}

#[cfg(feature = "otp")]
fn activity_log_attached(java_vm: &JavaVM, activity: &Global<JObject<'static>>, message: &str) {
    if let Err(error) = java_vm.attach_current_thread(|env| -> jni::errors::Result<()> {
        activity_log(env, activity.as_obj(), message)
    }) {
        log::warn!(target: "waterkit-otp", "failed to update manual OTP UI log: {error}");
    }
}

#[cfg(feature = "otp")]
fn activity_finish_otp_attached(
    java_vm: &JavaVM,
    activity: &Global<JObject<'static>>,
) -> jni::errors::Result<()> {
    java_vm.attach_current_thread(|env| activity_finish_otp(env, activity.as_obj()))
}

struct AndroidContextOwner {
    _activity: Global<JObject<'static>>,
}

static ANDROID_CONTEXT_ACTIVE: AtomicBool = AtomicBool::new(false);

enum AndroidContextOwnerError {
    AlreadyActive,
    Jni(jni::errors::Error),
}

impl AndroidContextOwner {
    fn new(env: &Env<'_>, activity: &JObject<'_>) -> Result<Self, AndroidContextOwnerError> {
        if ANDROID_CONTEXT_ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(AndroidContextOwnerError::AlreadyActive);
        }

        let result = (|| -> jni::errors::Result<Self> {
            let java_vm = env.get_java_vm()?;
            let activity = env.new_global_ref(activity)?;
            // SAFETY: both pointers are retained for this owner's lifetime,
            // and the active-owner guard prevents concurrent initialization.
            unsafe {
                ndk_context::initialize_android_context(
                    java_vm.get_raw().cast(),
                    activity.as_obj().as_raw().cast(),
                );
            }
            Ok(Self {
                _activity: activity,
            })
        })();

        result.map_err(|error| {
            ANDROID_CONTEXT_ACTIVE.store(false, Ordering::Release);
            AndroidContextOwnerError::Jni(error)
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
        ANDROID_CONTEXT_ACTIVE.store(false, Ordering::Release);
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

fn run_native_report(
    env: &mut Env<'_>,
    activity: &JObject<'_>,
    sms_delivery: bool,
    interactive: bool,
) -> TestReport {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime for Android test harness");
    let report = TestReport::new("android", "waterkit-test-android");
    let mut report = match Harness::new(env, activity, sms_delivery, interactive, &runtime, report)
    {
        Ok(harness) => harness.run(),
        Err(report) => report,
    };

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

#[cfg(feature = "dialog")]
static ACTIVITY_RESULT_ECHO: waterkit_build::DexHelper =
    waterkit_build::dex_helper!("com.waterkit.test.ActivityResultEchoActivity");

#[cfg(feature = "dialog")]
#[expect(
    clippy::future_not_send,
    reason = "the JNI environment belongs to the harness thread, which the current-thread runtime blocks on"
)]
async fn echo_activity_result(
    env: &mut Env<'_>,
    activity: &JObject<'_>,
    token: &str,
    result_code: i32,
    use_intent_sender: bool,
) -> Result<(waterkit_build::ResultCode, String), String> {
    let helper_class = ACTIVITY_RESULT_ECHO
        .class(env, activity)
        .map_err(|error| error.to_string())?;
    let token_string = env.new_string(token).map_err(|error| error.to_string())?;
    let arguments = [
        JValue::Object(activity),
        JValue::Object(&token_string),
        JValue::Int(result_code),
    ];
    let input = if use_intent_sender {
        env.call_static_method(
            helper_class,
            jni::jni_str!("intentSender"),
            jni::jni_sig!(
                "(Landroid/content/Context;Ljava/lang/String;I)Landroid/content/IntentSender;"
            ),
            &arguments,
        )
    } else {
        env.call_static_method(
            helper_class,
            jni::jni_str!("intent"),
            jni::jni_sig!("(Landroid/content/Context;Ljava/lang/String;I)Landroid/content/Intent;"),
            &arguments,
        )
    }
    .map_err(|error| error.to_string())?
    .l()
    .map_err(|error| error.to_string())?;
    let result = if use_intent_sender {
        waterkit_build::start_intent_sender_for_result(env, &input)
    } else {
        waterkit_build::start_activity_for_result(env, &input)
    }
    .map_err(|error| error.to_string())?
    .await
    .map_err(|error| error.to_string())?;
    let result_code = result.code();
    let data = result
        .into_data()
        .ok_or_else(|| "echo activity returned no Intent data".to_owned())?;
    let helper_class = ACTIVITY_RESULT_ECHO
        .class(env, activity)
        .map_err(|error| error.to_string())?;
    let token = env
        .call_static_method(
            helper_class,
            jni::jni_str!("token"),
            jni::jni_sig!("(Landroid/content/Intent;)Ljava/lang/String;"),
            &[JValue::Object(data.as_obj())],
        )
        .map_err(|error| error.to_string())?
        .l()
        .map_err(|error| error.to_string())?;
    let token = waterkit_build::decode_optional_string(env, &token)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "echo activity returned no token".to_owned())?;
    Ok((result_code, token))
}

#[cfg(feature = "dialog")]
#[expect(
    clippy::future_not_send,
    reason = "the JNI environment belongs to the harness thread, which the current-thread runtime blocks on"
)]
async fn record_android_activity_results(
    report: &mut TestReport,
    env: &mut Env<'_>,
    activity: &JObject<'_>,
    interactive: bool,
) {
    for (name, token, code, intent_sender) in [
        ("activity_result.intent", "activity-intent", -1, false),
        (
            "activity_result.intent_sender",
            "activity-intent-sender",
            -1,
            true,
        ),
        ("activity_result.custom_code", "activity-custom", 5, false),
    ] {
        match echo_activity_result(env, activity, token, code, intent_sender).await {
            Ok((waterkit_build::ResultCode::Ok, returned_token))
                if code == -1 && returned_token == token =>
            {
                report.push(TestCase::passed(name));
            }
            Ok((waterkit_build::ResultCode::Custom(5), returned_token))
                if code == 5 && returned_token == token =>
            {
                report.push(TestCase::passed(name));
            }
            Ok((result_code, returned_token)) => report.push(TestCase::failed(
                name,
                format!("unexpected result code or token: {result_code:?}, {returned_token}"),
            )),
            Err(error) => report.push(TestCase::failed(name, error)),
        }
    }

    let launch_failure = (|| -> Result<_, String> {
        let class = env
            .find_class(jni::jni_str!("android/content/Intent"))
            .map_err(|error| error.to_string())?;
        let intent = env
            .new_object(class, jni::jni_sig!("()V"), &[])
            .map_err(|error| error.to_string())?;
        let action = env
            .new_string("waterkit.test.NONEXISTENT_ACTION")
            .map_err(|error| error.to_string())?;
        env.call_method(
            &intent,
            jni::jni_str!("setAction"),
            jni::jni_sig!("(Ljava/lang/String;)Landroid/content/Intent;"),
            &[JValue::Object(&action)],
        )
        .map_err(|error| error.to_string())?;
        Ok(intent)
    })();
    match launch_failure {
        Ok(intent) => match waterkit_build::start_activity_for_result(env, &intent) {
            Ok(pending) => match pending.await {
                Err(waterkit_build::ActivityResultError::Launch(_)) => {
                    report.push(TestCase::passed("activity_result.launch_failure"));
                }
                Err(error) => report.push(TestCase::failed(
                    "activity_result.launch_failure",
                    format!("launch returned the wrong error: {error}"),
                )),
                Ok(_) => report.push(TestCase::failed(
                    "activity_result.launch_failure",
                    "nonexistent action unexpectedly returned an activity result",
                )),
            },
            Err(error) => report.push(TestCase::failed(
                "activity_result.launch_failure",
                format!("failed before asynchronous launch: {error}"),
            )),
        },
        Err(error) => report.push(TestCase::failed(
            "activity_result.launch_failure",
            format!("could not create invalid intent: {error}"),
        )),
    }

    if interactive {
        record_interactive_dialog_cases(report).await;
    }
}

#[cfg(feature = "dialog")]
async fn record_interactive_dialog_cases(report: &mut TestReport) {
    record_photo_picker_case(report).await;
    record_file_picker_case(report).await;
    record_multiple_file_picker_case(report).await;
    record_picker_cancelled_case(report).await;
}

#[cfg(feature = "dialog")]
async fn record_photo_picker_case(report: &mut TestReport) {
    use waterkit_content::dialog::{MediaType, PhotoPicker};

    match PhotoPicker::new()
        .with_media_type(MediaType::Image)
        .pick()
        .await
    {
        Ok(Some(handle)) => match handle.load().await {
            Ok(path) => match std::fs::read(&path) {
                Ok(bytes) if bytes.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10]) => {
                    report.push(TestCase::passed_with_message(
                        "dialog.photo_picker",
                        format!("path={} bytes={}", path.display(), bytes.len()),
                    ));
                }
                Ok(bytes) => report.push(TestCase::failed(
                    "dialog.photo_picker",
                    format!(
                        "selected image was not a PNG: path={} bytes={}",
                        path.display(),
                        bytes.len()
                    ),
                )),
                Err(error) => report.push(TestCase::failed(
                    "dialog.photo_picker",
                    format!("could not read selected image {}: {error}", path.display()),
                )),
            },
            Err(error) => report.push(TestCase::failed(
                "dialog.photo_picker",
                format!("could not load selected image: {error}"),
            )),
        },
        Ok(None) => report.push(TestCase::failed(
            "dialog.photo_picker",
            "photo picker returned no selection",
        )),
        Err(error) => report.push(TestCase::failed(
            "dialog.photo_picker",
            format!("photo picker failed: {error}"),
        )),
    }
}

#[cfg(feature = "dialog")]
async fn record_file_picker_case(report: &mut TestReport) {
    use waterkit_content::dialog::FileDialog;

    let selected_file = FileDialog::new()
        .with_filter("Text files", &["txt"])
        .pick_single()
        .await;
    match selected_file {
        Ok(Some(path)) => match std::fs::read_to_string(&path) {
            Ok(contents) if contents == "waterkit activity results a" => {
                report.push(TestCase::passed_with_message(
                    "dialog.file_picker",
                    format!("path={}", path.display()),
                ));
            }
            Ok(_) => report.push(TestCase::failed(
                "dialog.file_picker",
                format!(
                    "selected file did not contain the expected fixture: {}",
                    path.display()
                ),
            )),
            Err(error) => report.push(TestCase::failed(
                "dialog.file_picker",
                format!("could not read selected file {}: {error}", path.display()),
            )),
        },
        Ok(None) => report.push(TestCase::failed(
            "dialog.file_picker",
            "file picker returned no selection",
        )),
        Err(error) => report.push(TestCase::failed(
            "dialog.file_picker",
            format!("file picker failed: {error}"),
        )),
    }
}

#[cfg(feature = "dialog")]
async fn record_multiple_file_picker_case(report: &mut TestReport) {
    use waterkit_content::dialog::FileDialog;

    let selected_files = FileDialog::new()
        .with_filter("Text files", &["txt"])
        .pick_multiple()
        .await;
    match selected_files {
        Ok(Some(paths)) => {
            let mut contents = paths
                .iter()
                .map(std::fs::read_to_string)
                .collect::<Result<Vec<_>, _>>();
            match &mut contents {
                Ok(contents) => {
                    contents.sort();
                    let mut expected = vec![
                        "waterkit activity results a".to_owned(),
                        "waterkit activity results b".to_owned(),
                    ];
                    expected.sort();
                    if paths.len() == 2 && *contents == expected {
                        report.push(TestCase::passed_with_message(
                            "dialog.file_picker_multiple",
                            format!("selected={}", paths.len()),
                        ));
                    } else {
                        report.push(TestCase::failed(
                            "dialog.file_picker_multiple",
                            format!("unexpected fixture selection: {contents:?}"),
                        ));
                    }
                }
                Err(error) => report.push(TestCase::failed(
                    "dialog.file_picker_multiple",
                    format!("could not read selected fixtures: {error}"),
                )),
            }
        }
        Ok(None) => report.push(TestCase::failed(
            "dialog.file_picker_multiple",
            "multiple file picker returned no selection",
        )),
        Err(error) => report.push(TestCase::failed(
            "dialog.file_picker_multiple",
            format!("multiple file picker failed: {error}"),
        )),
    }
}

#[cfg(feature = "dialog")]
async fn record_picker_cancelled_case(report: &mut TestReport) {
    use waterkit_content::dialog::FileDialog;

    match FileDialog::new()
        .with_filter("Text files", &["txt"])
        .pick_single()
        .await
    {
        Ok(None) => report.push(TestCase::passed("dialog.picker_cancelled")),
        Ok(Some(_)) => report.push(TestCase::failed(
            "dialog.picker_cancelled",
            "picker returned a selection instead of being cancelled",
        )),
        Err(error) => report.push(TestCase::failed(
            "dialog.picker_cancelled",
            format!("picker cancellation failed: {error}"),
        )),
    }
}

#[cfg(feature = "language")]
async fn record_android_language(report: &mut TestReport) {
    use waterkit_content::language::translation;

    let capabilities = match translation::capabilities().await {
        Ok(capabilities) => capabilities,
        Err(error) => {
            record_android_language_query_failure(report, &error);
            return;
        }
    };
    record_android_capabilities(report, &capabilities);
    record_android_translation(report, &capabilities).await;
    record_android_not_installed(report, &capabilities).await;
    record_android_unsupported_pair(report).await;
    record_android_capability_updates(report);
}

#[cfg(feature = "language")]
fn record_android_language_query_failure(
    report: &mut TestReport,
    error: &waterkit_content::language::translation::TranslationError,
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
    report.push(TestCase::skipped(
        "language.unsupported_pair",
        "capability query failed",
    ));
}

#[cfg(feature = "language")]
fn record_android_capabilities(
    report: &mut TestReport,
    capabilities: &waterkit_content::language::translation::TranslationCapabilities,
) {
    let pairs = capabilities
        .pairs()
        .iter()
        .map(|capability| format!("{}={:?}", capability.pair(), capability.status()))
        .collect::<Vec<_>>();
    report.push(TestCase::passed_with_message(
        "language.capabilities",
        if pairs.is_empty() {
            "0 pairs (no on-device translation service in this image)".into()
        } else {
            pairs.join(", ")
        },
    ));
}

#[cfg(feature = "language")]
async fn record_android_translation(
    report: &mut TestReport,
    capabilities: &waterkit_content::language::translation::TranslationCapabilities,
) {
    use waterkit_content::language::translation::{AssetStatus, Translator};

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
async fn record_android_not_installed(
    report: &mut TestReport,
    capabilities: &waterkit_content::language::translation::TranslationCapabilities,
) {
    use waterkit_content::language::translation::{AssetStatus, TranslationError, Translator};

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

#[cfg(feature = "language")]
async fn record_android_unsupported_pair(report: &mut TestReport) {
    use waterkit_content::language::translation::{TranslationError, Translator};

    let api_level = match android_api_level() {
        Ok(level) => level,
        Err(error) => {
            report.push(TestCase::failed(
                "language.unsupported_pair",
                format!("could not read Android API level: {error}"),
            ));
            return;
        }
    };
    match Translator::new(
        waterkit_content::language::langid!("en"),
        waterkit_content::language::langid!("en"),
    )
    .await
    {
        Err(TranslationError::UnsupportedPair(pair)) if api_level >= 31 => {
            report.push(TestCase::passed_with_message(
                "language.unsupported_pair",
                format!("{pair} is unsupported"),
            ));
        }
        Err(TranslationError::Unavailable) if api_level < 31 => {
            report.push(TestCase::passed_with_message(
                "language.unsupported_pair",
                "translation unavailable below API 31",
            ));
        }
        Err(error) => report.push(TestCase::failed(
            "language.unsupported_pair",
            format!("unexpected result at API {api_level}: {error}"),
        )),
        Ok(_) => report.push(TestCase::failed(
            "language.unsupported_pair",
            format!("en to en unexpectedly created a translator at API {api_level}"),
        )),
    }
}

#[cfg(feature = "language")]
fn record_android_capability_updates(report: &mut TestReport) {
    use waterkit_content::language::translation::{self, TranslationError};

    let api_level = match android_api_level() {
        Ok(level) => level,
        Err(error) => {
            report.push(TestCase::failed(
                "language.capability_updates",
                format!("could not read Android API level: {error}"),
            ));
            return;
        }
    };

    match (api_level >= 31, translation::android::capability_updates()) {
        (true, Ok(updates)) => {
            drop(updates);
            report.push(TestCase::passed_with_message(
                "language.capability_updates",
                "registered and removed capability listener",
            ));
        }
        (false, Err(TranslationError::Unavailable)) => {
            report.push(TestCase::passed_with_message(
                "language.capability_updates",
                "translation unavailable below API 31",
            ));
        }
        // `Unavailable` at a supported API level means the device has no
        // system translation service — the same state `capabilities()` reports
        // as zero pairs, and not a registration failure.
        (true, Err(TranslationError::Unavailable)) => report.push(TestCase::skipped(
            "language.capability_updates",
            "no on-device translation service on this device",
        )),
        (_, Ok(updates)) => {
            drop(updates);
            report.push(TestCase::failed(
                "language.capability_updates",
                format!("capability listener unexpectedly registered at API {api_level}"),
            ));
        }
        (_, Err(error)) => report.push(TestCase::failed(
            "language.capability_updates",
            format!("could not register capability listener at API {api_level}: {error}"),
        )),
    }
}

#[cfg(feature = "language")]
fn android_api_level() -> Result<i32, String> {
    use jni::{jni_sig, jni_str};

    waterkit_build::with_android_context(|env, _context| {
        let sdk_version = env.get_static_field(
            jni_str!("android/os/Build$VERSION"),
            jni_str!("SDK_INT"),
            jni_sig!("I"),
        )?;
        Ok::<_, waterkit_build::AndroidError>(sdk_version.i()?)
    })
    .map_err(|error| error.to_string())
}

/// What every capability's recorder shares: the JNI environment, a global
/// reference to the activity, the files directory, the runtime its
/// asynchronous calls run on, and the report its cases go into.
struct Harness<'h, 'local> {
    #[cfg_attr(
        not(any(
            feature = "sensor",
            feature = "location",
            feature = "permission",
            feature = "fs",
            feature = "secret",
            feature = "clipboard",
            feature = "otp",
            feature = "dialog",
            feature = "vision"
        )),
        expect(
            dead_code,
            reason = "only the recorders that call into JNI read the environment"
        )
    )]
    env: &'h mut Env<'local>,
    #[cfg_attr(
        not(any(
            feature = "sensor",
            feature = "location",
            feature = "permission",
            feature = "fs",
            feature = "secret",
            feature = "clipboard",
            feature = "otp",
            feature = "dialog"
        )),
        expect(
            dead_code,
            reason = "only the recorders that call into the activity read it"
        )
    )]
    activity: Global<JObject<'static>>,
    #[cfg(feature = "camera")]
    files_dir: std::path::PathBuf,
    #[cfg(feature = "otp")]
    sms_delivery: bool,
    #[cfg(feature = "dialog")]
    interactive: bool,
    runtime: &'h tokio::runtime::Runtime,
    report: TestReport,
}

impl<'h, 'local> Harness<'h, 'local> {
    /// Sets up what the recorders share, or returns `report` with the case
    /// that says which part could not be set up.
    fn new(
        env: &'h mut Env<'local>,
        activity: &JObject<'_>,
        sms_delivery: bool,
        interactive: bool,
        runtime: &'h tokio::runtime::Runtime,
        mut report: TestReport,
    ) -> Result<Self, TestReport> {
        #[cfg(not(feature = "otp"))]
        let _ = sms_delivery;
        #[cfg(not(feature = "dialog"))]
        let _ = interactive;
        let global_activity = match env.new_global_ref(activity) {
            Ok(value) => value,
            Err(error) => {
                report.push(TestCase::failed(
                    "harness.activity_ref",
                    format!("failed to create global activity ref: {error}"),
                ));
                return Err(report);
            }
        };
        #[cfg(feature = "camera")]
        let files_dir = match files_dir(env, activity) {
            Ok(dir) => dir,
            Err(error) => {
                report.push(TestCase::failed("harness.files_dir", error.to_string()));
                return Err(report);
            }
        };
        Ok(Self {
            env,
            activity: global_activity,
            #[cfg(feature = "camera")]
            files_dir,
            #[cfg(feature = "otp")]
            sms_delivery,
            #[cfg(feature = "dialog")]
            interactive,
            runtime,
            report,
        })
    }

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
type Recorder = fn(&mut Harness<'_, '_>);

/// The recorder of every enabled capability, in report order. The features
/// this harness only links, or cannot exercise without an interactive prompt
/// or the user's data, record a fixed case.
const RECORDERS: &[Recorder] = &[
    #[cfg(feature = "sensor")]
    |h| record_android_sensor(&mut h.report, h.env, h.activity.as_obj()),
    #[cfg(feature = "location")]
    |h| record_android_location(&mut h.report, h.env, h.activity.as_obj()),
    #[cfg(feature = "permission")]
    |h| record_android_permission(&mut h.report, h.env, h.activity.as_obj()),
    #[cfg(feature = "camera")]
    |h| {
        h.runtime
            .block_on(record_android_camera(&mut h.report, &h.files_dir));
    },
    #[cfg(feature = "vision")]
    |h| {
        // The text still renders through the platform's rasterizer, which
        // needs the JNIEnv; keeping the call synchronous keeps every case
        // future Send.
        let text_still = text_png(h.env, "WATERKIT 137").map_err(|error| error.to_string());
        let files_dir = h.files_dir.clone();
        h.runtime.block_on(record_android_vision_requests(
            &mut h.report,
            &files_dir,
            text_still,
        ));
    },
    #[cfg(feature = "clipboard")]
    |h| h.runtime.block_on(record_android_clipboard(&mut h.report)),
    #[cfg(feature = "clipboard")]
    |h| {
        let case = h
            .runtime
            .block_on(ClipboardFiles::record(h.env, h.activity.as_obj()));
        h.report.push(case);
    },
    #[cfg(feature = "fs")]
    |h| record_android_fs(&mut h.report, h.env, h.activity.as_obj()),
    #[cfg(feature = "haptic")]
    |h| h.runtime.block_on(record_android_haptic(&mut h.report)),
    #[cfg(feature = "notification")]
    |h| {
        h.runtime
            .block_on(record_android_notification(&mut h.report));
    },
    #[cfg(feature = "secret")]
    |h| record_android_secret(&mut h.report, h.env, h.activity.as_obj()),
    #[cfg(feature = "system")]
    |h| h.runtime.block_on(record_android_system(&mut h.report)),
    #[cfg(feature = "background")]
    |h| record_android_background(&mut h.report),
    #[cfg(feature = "passkey")]
    |h| h.runtime.block_on(record_android_passkey(&mut h.report)),
    #[cfg(feature = "otp")]
    |h| {
        let files_dir = android_files_dir(h.env, h.activity.as_obj());
        h.runtime
            .block_on(record_android_otp(&mut h.report, h.sms_delivery, files_dir));
    },
    #[cfg(feature = "codec")]
    |h| record_android_avif_decode(&mut h.report),
    #[cfg(feature = "health")]
    |h| record_android_health(&mut h.report),
    #[cfg(feature = "language")]
    |h| h.runtime.block_on(record_android_language(&mut h.report)),
    #[cfg(feature = "wallet")]
    |h| h.runtime.block_on(record_android_wallet(&mut h.report)),
    #[cfg(feature = "screen")]
    |h| record_android_screen(&mut h.report),
    #[cfg(feature = "dialog")]
    |h| {
        h.runtime.block_on(record_android_activity_results(
            &mut h.report,
            h.env,
            h.activity.as_obj(),
            h.interactive,
        ));
    },
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
    #[cfg(feature = "video")]
    |h| h.report.push(TestCase::passed("video.linked")),
    #[cfg(feature = "bluetooth")]
    |h| h.report.push(TestCase::passed("bluetooth.linked")),
    #[cfg(feature = "nfc")]
    |h| h.report.push(TestCase::passed("nfc.linked")),
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
    #[cfg(feature = "deeplink")]
    |h| h.report.push(TestCase::passed("deeplink.linked")),
    #[cfg(feature = "vision")]
    |h| h.runtime.block_on(record_android_vision(&mut h.report)),
];

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
                record_android_camera_analysis(report, &camera).await;
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
#[expect(
    clippy::too_many_lines,
    reason = "one streaming pass over the camera's frames with its summary checks; splitting it would scatter a single case"
)]
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
    let opened = Instant::now();
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
        timestamps,
    } = summary;
    // A capture span cannot exceed the time since the camera was opened.
    let span = timestamps.last;
    if let Some(span) = span
        && span > opened.elapsed()
    {
        report.push(TestCase::failed(
            case,
            format!("last frame timestamp {span:?} exceeds time since open"),
        ));
        return;
    }
    if let Some((index, at)) = timestamps.first_unordered {
        report.push(TestCase::failed(
            case,
            format!("frame {index} timestamp {at:?} is not strictly greater than the previous"),
        ));
        return;
    }
    report.push(TestCase::passed_with_message(
        case,
        format!(
            "front={} frames={count} fps={:.1} planes={layouts:?} stored={}x{} orientations={orientations:?} upright={}x{} span={:?} png={}",
            camera.is_front_facing,
            f64::from(count) / elapsed,
            stored.0,
            stored.1,
            upright.width(),
            upright.height(),
            span.unwrap_or_default(),
            png.display(),
        ),
    ));
    record_android_camera_reopen(report, camera, &device, &queue).await;
}

/// Checks that two consecutive analysis frames carry an advancing stream
/// clock, a size, a live `Image` and planes large enough for their strides.
#[cfg(feature = "camera")]
fn check_analysis_frames(
    frame: &waterkit_content::camera::AnalysisFrame,
    second: &waterkit_content::camera::AnalysisFrame,
) -> Result<(), String> {
    if second.timestamp().is_zero() || second.timestamp() <= frame.timestamp() {
        return Err(format!(
            "analysis timestamps do not advance: {:?} then {:?}",
            frame.timestamp(),
            second.timestamp()
        ));
    }
    if frame.width() == 0 || frame.height() == 0 {
        return Err("analysis frame has no size".to_owned());
    }
    if frame.media_image().as_obj().is_null() {
        return Err("analysis frame holds a null image".to_owned());
    }
    let planes = frame.planes();
    let chroma_width = frame.width().div_ceil(2) as usize;
    let chroma_height = frame.height().div_ceil(2) as usize;
    for (name, plane, width, height) in [
        (
            "luma",
            &planes.luma,
            frame.width() as usize,
            frame.height() as usize,
        ),
        ("cb", &planes.cb, chroma_width, chroma_height),
        ("cr", &planes.cr, chroma_width, chroma_height),
    ] {
        let need = plane.row_stride() * (height - 1) + plane.pixel_stride() * (width - 1) + 1;
        if plane.bytes().len() < need {
            return Err(format!(
                "{name} plane has {} bytes, needs {need}",
                plane.bytes().len()
            ));
        }
    }
    Ok(())
}

/// Opens `camera` with an analysis output and takes its first two analysis
/// frames, then one preview frame while the analysis stream is open — the
/// GPU preview must keep streaming next to the `YUV_420_888` reader.
#[cfg(feature = "camera")]
async fn record_android_camera_analysis(
    report: &mut TestReport,
    camera: &waterkit_content::camera::CameraInfo,
) {
    use futures::StreamExt;
    use std::sync::Arc;
    use std::time::Duration;
    use waterkit_content::camera::{AnalysisConfig, Camera, CameraConfig};

    let case = format!("camera.analysis.{}", camera.id);
    let (device, queue) = match camera_gpu().await {
        Ok(gpu) => gpu,
        Err(error) => {
            report.push(TestCase::failed(case, error));
            return;
        }
    };
    let handle = match Camera::open(
        &camera.id,
        CameraConfig {
            analysis: Some(AnalysisConfig::default()),
            ..CameraConfig::default()
        },
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

    let mut analysis = std::pin::pin!(handle.analysis_frames());
    let frame = match next_analysis_frame(
        &case,
        0,
        tokio::time::timeout(Duration::from_secs(5), analysis.next()).await,
    ) {
        Ok(frame) => frame,
        Err(outcome) => {
            report.push(outcome);
            return;
        }
    };
    // The stream clock reads zero on the first analysis frame, like
    // `Frame::timestamp`; the second frame must carry a later, nonzero one.
    let second = match next_analysis_frame(
        &case,
        1,
        tokio::time::timeout(Duration::from_secs(5), analysis.next()).await,
    ) {
        Ok(frame) => frame,
        Err(outcome) => {
            report.push(outcome);
            return;
        }
    };
    if let Err(error) = check_analysis_frames(&frame, &second) {
        report.push(TestCase::failed(case, error));
        return;
    }
    let planes = frame.planes();

    // A preview frame while the analysis stream stays open proves the
    // GPU stream keeps running next to the YUV_420_888 reader.
    let mut frames = std::pin::pin!(handle.frames());
    let preview = match next_frame(
        &case,
        0,
        tokio::time::timeout(Duration::from_secs(5), frames.next()).await,
    ) {
        Ok(frame) => frame,
        Err(outcome) => {
            report.push(outcome);
            return;
        }
    };

    report.push(TestCase::passed_with_message(
        case,
        format!(
            "{}x{} strides luma={}x{} cb={}x{} cr={}x{} ts={:?} preview={}x{}@{:?}",
            frame.width(),
            frame.height(),
            planes.luma.row_stride(),
            planes.luma.pixel_stride(),
            planes.cb.row_stride(),
            planes.cb.pixel_stride(),
            planes.cr.row_stride(),
            planes.cr.pixel_stride(),
            second.timestamp(),
            preview.width(),
            preview.height(),
            preview.timestamp(),
        ),
    ));
}

/// Reopens `camera` right after its frames case dropped the handle, and
/// takes one frame from each open. Dropping a camera joins its teardown, so
/// the immediate second open passes only when teardown already finished.
#[cfg(feature = "camera")]
async fn record_android_camera_reopen(
    report: &mut TestReport,
    camera: &waterkit_content::camera::CameraInfo,
    device: &std::sync::Arc<waterkit_content::camera::wgpu::Device>,
    queue: &std::sync::Arc<waterkit_content::camera::wgpu::Queue>,
) {
    use futures::StreamExt;
    use std::time::Duration;
    use waterkit_content::camera::{Camera, CameraConfig};

    let case = format!("camera.reopen.{}", camera.id);
    for open in 0..2 {
        let handle = match Camera::open(
            &camera.id,
            CameraConfig::default(),
            std::sync::Arc::clone(device),
            std::sync::Arc::clone(queue),
        )
        .await
        {
            Ok(handle) => handle,
            Err(error) => {
                report.push(TestCase::failed(
                    case,
                    format!("open {open} failed: {error}"),
                ));
                return;
            }
        };
        {
            let mut frames = std::pin::pin!(handle.frames());
            let next = tokio::time::timeout(Duration::from_secs(5), frames.next()).await;
            match next_frame(&case, 0, next) {
                Ok(frame) => drop(frame),
                Err(outcome) => {
                    report.push(outcome);
                    return;
                }
            }
        }
        drop(handle);
    }
    report.push(TestCase::passed(case));
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

/// The next frame of a camera stream, or the failure that ends the case after
/// `count` frames: a device opened with the import's requirements converts
/// every camera buffer, so any end of the stream, an import error included,
/// fails the case.
#[cfg(feature = "camera")]
fn next_frame(
    case: &str,
    count: u32,
    next: Result<
        Option<Result<waterkit_content::camera::Frame, waterkit_content::camera::CameraError>>,
        tokio::time::error::Elapsed,
    >,
) -> Result<waterkit_content::camera::Frame, TestCase> {
    match next {
        Ok(Some(Ok(frame))) => Ok(frame),
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

/// The next analysis frame, or the failure that ends the case after `count`
/// frames; the analysis stream ends only when the camera's capture does.
#[cfg(feature = "camera")]
fn next_analysis_frame(
    case: &str,
    count: u32,
    next: Result<
        Option<
            Result<waterkit_content::camera::AnalysisFrame, waterkit_content::camera::CameraError>,
        >,
        tokio::time::error::Elapsed,
    >,
) -> Result<waterkit_content::camera::AnalysisFrame, TestCase> {
    match next {
        Ok(Some(Ok(frame))) => Ok(frame),
        Ok(Some(Err(error))) => Err(TestCase::failed(
            case,
            format!("analysis stream failed after {count} frames: {error}"),
        )),
        Ok(None) => Err(TestCase::failed(
            case,
            format!("analysis stream ended after {count} frames"),
        )),
        Err(_) => Err(TestCase::failed(
            case,
            format!("no analysis frame within 5 s after {count}"),
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
    /// Frame timestamps seen, on the camera's capture clock.
    timestamps: Timestamps,
}

/// What the frame timestamps showed: monotonic, from the first captured
/// frame.
#[cfg(feature = "camera")]
#[derive(Default)]
struct Timestamps {
    /// The last frame's timestamp; the stream's capture span.
    last: Option<std::time::Duration>,
    /// The first frame whose timestamp did not strictly increase, as
    /// (frame index, its timestamp).
    first_unordered: Option<(u32, std::time::Duration)>,
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
        let at = frame.timestamp();
        if let Some(previous) = self.timestamps.last
            && at <= previous
            && self.timestamps.first_unordered.is_none()
        {
            self.timestamps.first_unordered = Some((self.count, at));
        }
        self.timestamps.last = Some(at);
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

    let mut primary_stream = match clipboard.watch().await {
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
    let mut second_stream = match clipboard.watch().await {
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
    match clipboard.watch().await {
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
async fn record_android_haptic(report: &mut TestReport) {
    match waterkit_content::haptic::Haptic::impact(waterkit_content::haptic::Intensity::LOW).await {
        Ok(()) => report.push(TestCase::passed("haptic.impact")),
        Err(error) => report.push(TestCase::failed(
            "haptic.impact",
            format!("haptic impact failed: {error}"),
        )),
    }
}

#[cfg(feature = "notification")]
async fn record_android_notification(report: &mut TestReport) {
    let result = waterkit_content::notification::Notification::new()
        .title("WaterKit Android Harness")
        .body("notification test")
        .show()
        .await;
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
async fn record_android_system(report: &mut TestReport) {
    use waterkit_content::system;

    report.push(match system::connectivity().await {
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
    report.push(match system::load().await {
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

#[cfg(feature = "otp")]
async fn record_android_otp(
    report: &mut TestReport,
    sms_delivery: bool,
    files_dir: Result<std::path::PathBuf, String>,
) {
    use waterkit_content::otp::AddressedRequest;

    let capabilities = match waterkit_content::otp::capabilities() {
        Ok(capabilities) => capabilities,
        Err(error) => {
            report.push(TestCase::failed(
                "otp.capabilities",
                format!("capability query failed: {error}"),
            ));
            report.push(TestCase::skipped(
                "otp.addressed",
                "capability query failed",
            ));
            report.push(TestCase::skipped("otp.consent", "capability query failed"));
            return;
        }
    };
    let capability_message = otp_capability_message(capabilities);
    if capabilities.addressed.is_some() {
        report.push(TestCase::passed_with_message(
            "otp.capabilities",
            capability_message,
        ));
    } else {
        report.push(TestCase::failed(
            "otp.capabilities",
            format!("addressed retrieval is unavailable; {capability_message}"),
        ));
    }

    if sms_delivery {
        let realization = capabilities.addressed.map_or_else(
            || "unavailable".to_owned(),
            |realization| format!("{realization:?}"),
        );
        match AddressedRequest::start().await {
            Err(error) => report.push(TestCase::failed(
                "otp.addressed",
                format!("start failed (realization={realization}): {error}"),
            )),
            Ok(request) => {
                let body = format!("Your waterkit code is 123456 {}", request.token());
                let files_dir = match files_dir {
                    Ok(path) => path,
                    Err(error) => {
                        report.push(TestCase::failed(
                            "otp.addressed",
                            format!("could not resolve activity filesDir: {error}"),
                        ));
                        return;
                    }
                };
                if let Err(error) = write_sms_request(files_dir, "5551234", &body).await {
                    report.push(TestCase::failed(
                        "otp.addressed",
                        format!("could not publish test SMS request: {error}"),
                    ));
                    return;
                }

                match tokio::time::timeout(std::time::Duration::from_secs(30), request.message())
                    .await
                {
                    Ok(Ok(message)) if message == body => {
                        report.push(TestCase::passed_with_message(
                            "otp.addressed",
                            format!("received exact test SMS using {realization}"),
                        ));
                    }
                    Ok(Ok(_)) => report.push(TestCase::failed(
                        "otp.addressed",
                        format!(
                            "received text did not match the test SMS (realization={realization})"
                        ),
                    )),
                    Ok(Err(error)) => report.push(TestCase::failed(
                        "otp.addressed",
                        format!("message retrieval failed (realization={realization}): {error}"),
                    )),
                    Err(_) => report.push(TestCase::failed(
                        "otp.addressed",
                        format!("timed out waiting for the test SMS (realization={realization})"),
                    )),
                }
            }
        }
    } else {
        report.push(TestCase::skipped(
            "otp.addressed",
            "no host SMS delivery: physical device",
        ));
    }

    report.push(TestCase::skipped(
        "otp.consent",
        format!(
            "needs the user's consent tap; exercised manually; consent_available={}",
            capabilities.consent
        ),
    ));
}

#[cfg(feature = "health")]
fn record_android_health(report: &mut TestReport) {
    report.push(TestCase::passed_with_message(
        "health.availability",
        format!(
            "available={}",
            waterkit_content::health::capabilities().available
        ),
    ));
}

#[cfg(feature = "wallet")]
async fn record_android_wallet(report: &mut TestReport) {
    match waterkit_content::wallet::capabilities().await {
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

#[cfg(feature = "otp")]
fn otp_capability_message(capabilities: waterkit_content::otp::OtpCapabilities) -> String {
    format!(
        "available={} addressed={:?} consent={} one_time_code_autofill={}",
        capabilities.available,
        capabilities.addressed,
        capabilities.consent,
        capabilities.one_time_code_autofill
    )
}

#[cfg(feature = "otp")]
fn android_files_dir(
    env: &mut Env<'_>,
    activity: &JObject<'_>,
) -> Result<std::path::PathBuf, String> {
    let files_dir = env
        .call_method(
            activity,
            jni_str!("getFilesDir"),
            jni_sig!("()Ljava/io/File;"),
            &[],
        )
        .and_then(JValueOwned::l)
        .map_err(|error| describe_jni_error(env, error))?;
    let path = env
        .call_method(
            &files_dir,
            jni_str!("getAbsolutePath"),
            jni_sig!("()Ljava/lang/String;"),
            &[],
        )
        .and_then(JValueOwned::l)
        .map_err(|error| describe_jni_error(env, error))?;
    decode_string(env, &path)
        .map(std::path::PathBuf::from)
        .map_err(|error| error.to_string())
}

#[cfg(feature = "otp")]
async fn write_sms_request(
    files_dir: std::path::PathBuf,
    sender: &str,
    body: &str,
) -> Result<(), String> {
    #[derive(serde::Serialize)]
    struct SmsRequest<'a> {
        sender: &'a str,
        body: &'a str,
    }

    let request_path = files_dir.join("waterkit-sms-request.json");
    let temp_path = files_dir.join("waterkit-sms-request.json.tmp");
    let contents =
        serde_json::to_vec(&SmsRequest { sender, body }).map_err(|error| error.to_string())?;
    tokio::fs::write(&temp_path, contents)
        .await
        .map_err(|error| error.to_string())?;
    tokio::fs::rename(temp_path, request_path)
        .await
        .map_err(|error| error.to_string())
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

/// The payload the generated QR still carries.
#[cfg(feature = "vision")]
const VISION_QR_TEXT: &str = "waterkit vision #137";

/// Exercises the Android vision realization over ML Kit: a generated QR
/// still, a text still drawn by the device's rasterizer, a request for a
/// symbology ML Kit cannot express, one live camera frame through the
/// frame-plane `NV21` path, and the capabilities the device reports.
#[cfg(feature = "vision")]
async fn record_android_vision_requests(
    report: &mut TestReport,
    files_dir: &std::path::Path,
    text_still: Result<Vec<u8>, String>,
) {
    use std::sync::Arc;
    use waterkit_content::vision::Vision;

    let (device, queue) = match camera_gpu().await {
        Ok(gpu) => gpu,
        Err(error) => {
            report.push(TestCase::failed("vision.gpu", error));
            return;
        }
    };
    let vision = Vision::new(Arc::clone(&device), Arc::clone(&queue));
    // ML Kit is Play services' realization: a device without Play services
    // has none, and every request must say so instead of answering.
    if !waterkit_content::vision::CodeScanner::capabilities().available {
        record_vision_without_play_services(report, &vision).await;
        return;
    }
    match qr_png(VISION_QR_TEXT) {
        Ok(png) => {
            let _ = std::fs::write(files_dir.join("vision-qr.png"), &png);
            record_vision_barcodes(report, &vision, png).await;
        }
        Err(error) => report.push(TestCase::failed("vision.barcode.still", error)),
    }
    record_vision_text(report, &vision, text_still, files_dir).await;
    record_vision_frame(report, &vision, &device, &queue).await;
    record_vision_capabilities(report, &vision);
}

/// Barcode requests over the generated QR still: one served read, then a
/// symbology the serving realization cannot express, which must fail at
/// plan time naming it.
#[cfg(feature = "vision")]
async fn record_vision_barcodes(
    report: &mut TestReport,
    vision: &waterkit_content::vision::Vision,
    qr_png: Vec<u8>,
) {
    use waterkit_content::vision::{DetectBarcodes, EnumSet, Image, Symbology, VisionError};

    let image = Image::from_encoded(qr_png.into());
    match vision
        .perform(&image, &DetectBarcodes::new(EnumSet::only(Symbology::Qr)))
        .await
    {
        Ok(barcodes)
            if barcodes.len() == 1
                && barcodes[0].symbology() == Symbology::Qr
                && barcodes[0].payload().text() == Some(VISION_QR_TEXT) =>
        {
            report.push(TestCase::passed_with_message(
                "vision.barcode.still",
                format!("{barcodes:?}"),
            ));
        }
        Ok(barcodes) => report.push(TestCase::failed(
            "vision.barcode.still",
            format!("expected one QR reading '{VISION_QR_TEXT}': {barcodes:?}"),
        )),
        Err(error) => report.push(TestCase::failed(
            "vision.barcode.still",
            format!("detect failed: {error}"),
        )),
    }

    match vision
        .perform(
            &image,
            &DetectBarcodes::new(EnumSet::only(Symbology::MicroQr)),
        )
        .await
    {
        Err(VisionError::Unsupported(message)) if message.contains("MicroQr") => {
            report.push(TestCase::passed_with_message(
                "vision.barcode.unsupported",
                message,
            ));
        }
        outcome => report.push(TestCase::failed(
            "vision.barcode.unsupported",
            format!("expected Unsupported naming MicroQr: {outcome:?}"),
        )),
    }
}

/// Latin text over `text_still`, which the platform's own rasterizer drew.
#[cfg(feature = "vision")]
async fn record_vision_text(
    report: &mut TestReport,
    vision: &waterkit_content::vision::Vision,
    text_still: Result<Vec<u8>, String>,
    files_dir: &std::path::Path,
) {
    use waterkit_content::vision::{Image, RecognizeText};

    let png = match text_still {
        Ok(png) => png,
        Err(error) => {
            report.push(TestCase::failed(
                "vision.text.still",
                format!("rendering text still: {error}"),
            ));
            return;
        }
    };
    let _ = std::fs::write(files_dir.join("vision-text.png"), &png);
    let image = Image::from_encoded(png.into());
    match vision.perform(&image, &RecognizeText::new()).await {
        Ok(lines) if lines.iter().any(|line| line.text.contains("WATERKIT")) => {
            let texts: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
            report.push(TestCase::passed_with_message(
                "vision.text.still",
                format!("{texts:?}"),
            ));
        }
        Ok(lines) => report.push(TestCase::failed(
            "vision.text.still",
            format!("no line reads WATERKIT: {lines:?}"),
        )),
        Err(error) => report.push(TestCase::failed(
            "vision.text.still",
            format!("recognize failed: {error}"),
        )),
    }
}

/// The camera path: an analysis frame's `android.media.Image` reaches ML
/// Kit through `InputImage.fromMediaImage`, with no pixel copy. The
/// emulator's virtual scene may hold nothing, so an empty result still
/// passes.
#[cfg(feature = "vision")]
async fn record_vision_frame(
    report: &mut TestReport,
    vision: &waterkit_content::vision::Vision,
    device: &std::sync::Arc<waterkit_content::camera::wgpu::Device>,
    queue: &std::sync::Arc<waterkit_content::camera::wgpu::Queue>,
) {
    match camera_vision_frame(vision, device, queue).await {
        Ok((barcodes, lines)) => report.push(TestCase::passed_with_message(
            "vision.analysis_frame",
            format!("analysis frame served: barcodes={barcodes} lines={lines}"),
        )),
        Err(case) => report.push(case),
    }
}

/// What the device serves natively: ML Kit's symbology set and script
/// recognizers when Play services is present.
#[cfg(feature = "vision")]
/// On a device without Play services, barcode and text requests report that
/// no realization exists and the capabilities list no native realization.
async fn record_vision_without_play_services(
    report: &mut TestReport,
    vision: &waterkit_content::vision::Vision,
) {
    use waterkit_content::vision::{
        DetectBarcodes, EnumSet, Image, RecognizeText, Symbology, VisionError,
    };

    let png = match qr_png(VISION_QR_TEXT) {
        Ok(png) => png,
        Err(error) => {
            report.push(TestCase::failed("vision.barcode.still", error));
            return;
        }
    };
    let image = Image::from_encoded(png.into());
    match vision
        .perform(&image, &DetectBarcodes::new(EnumSet::only(Symbology::Qr)))
        .await
    {
        Err(VisionError::Unsupported(message)) => report.push(TestCase::passed_with_message(
            "vision.barcode.still",
            format!("unsupported without Play services: {message}"),
        )),
        outcome => report.push(TestCase::failed(
            "vision.barcode.still",
            format!("barcode detection without Play services returned {outcome:?}"),
        )),
    }
    match vision.perform(&image, &RecognizeText::new()).await {
        Err(VisionError::Unsupported(message)) => report.push(TestCase::passed_with_message(
            "vision.text.still",
            format!("unsupported without Play services: {message}"),
        )),
        outcome => report.push(TestCase::failed(
            "vision.text.still",
            format!("text recognition without Play services returned {outcome:?}"),
        )),
    }
    let capabilities = vision.capabilities();
    if capabilities.barcodes.native.is_empty() && capabilities.text.native.is_empty() {
        report.push(TestCase::passed_with_message(
            "vision.capabilities",
            "no native realization without Play services",
        ));
    } else {
        report.push(TestCase::failed(
            "vision.capabilities",
            format!(
                "without Play services: native barcodes={} scripts={}",
                capabilities.barcodes.native.len(),
                capabilities.text.native.len()
            ),
        ));
    }
}

#[cfg(feature = "vision")]
fn record_vision_capabilities(report: &mut TestReport, vision: &waterkit_content::vision::Vision) {
    let capabilities = vision.capabilities();
    let symbologies = capabilities.barcodes.native.len();
    let scripts = capabilities.text.native.len();
    let script_names = capabilities
        .text
        .native
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    if symbologies == 13 && scripts == 5 {
        report.push(TestCase::passed_with_message(
            "vision.capabilities",
            format!("barcodes={symbologies} scripts=[{script_names}]"),
        ));
    } else {
        report.push(TestCase::failed(
            "vision.capabilities",
            format!("native barcodes={symbologies} (want 13) scripts=[{script_names}] (want 5)"),
        ));
    }
}

/// One live analysis frame served to `vision`: barcode and text requests
/// together prove the frame's `android.media.Image` reaches ML Kit with no
/// pixel copy.
#[cfg(feature = "vision")]
async fn camera_vision_frame(
    vision: &waterkit_content::vision::Vision,
    device: &std::sync::Arc<waterkit_content::camera::wgpu::Device>,
    queue: &std::sync::Arc<waterkit_content::camera::wgpu::Queue>,
) -> Result<(usize, usize), TestCase> {
    use futures::StreamExt;
    use std::sync::Arc;
    use std::time::Duration;
    use waterkit_content::camera::{AnalysisConfig, Camera, CameraConfig};
    use waterkit_content::vision::{DetectBarcodes, EnumSet, Image, RecognizeText, Symbology};

    const CASE: &str = "vision.analysis_frame";
    let cameras = Camera::list()
        .map_err(|error| TestCase::failed(CASE, format!("camera list failed: {error}")))?;
    let Some(camera) = cameras.into_iter().next() else {
        return Err(TestCase::skipped(CASE, "no camera on this device"));
    };
    let handle = Camera::open(
        &camera.id,
        CameraConfig {
            analysis: Some(AnalysisConfig::default()),
            ..CameraConfig::default()
        },
        Arc::clone(device),
        Arc::clone(queue),
    )
    .await
    .map_err(|error| TestCase::failed(CASE, format!("open failed: {error}")))?;
    let mut frames = std::pin::pin!(handle.analysis_frames());
    let next = tokio::time::timeout(Duration::from_secs(10), frames.next()).await;
    let frame = next_analysis_frame(CASE, 0, next)?;
    let image = Image::from(&frame);
    vision
        .perform(
            &image,
            &(
                DetectBarcodes::new(EnumSet::only(Symbology::Qr)),
                RecognizeText::new(),
            ),
        )
        .await
        .map(|(barcodes, lines)| (barcodes.len(), lines.len()))
        .map_err(|error| TestCase::failed(CASE, format!("perform failed: {error}")))
}

/// Draws `payload` as a QR code and encodes it as PNG, so the harness ships
/// no barcode fixtures.
#[cfg(feature = "vision")]
fn qr_png(payload: &str) -> Result<Vec<u8>, String> {
    const QUIET: usize = 4;
    const SCALE: usize = 8;
    let qr = qrcodegen::QrCode::encode_text(payload, qrcodegen::QrCodeEcc::Medium)
        .map_err(|error| format!("encoding QR: {error}"))?;
    let side = (usize::try_from(qr.size()).map_err(|error| format!("QR size: {error}"))?
        + QUIET * 2)
        * SCALE;
    let mut rgba = vec![255_u8; side * side * 4];
    for module_y in 0..qr.size() {
        for module_x in 0..qr.size() {
            if !qr.get_module(module_x, module_y) {
                continue;
            }
            let base_x =
                (usize::try_from(module_x).expect("module coords are nonnegative") + QUIET) * SCALE;
            let base_y =
                (usize::try_from(module_y).expect("module coords are nonnegative") + QUIET) * SCALE;
            for dy in 0..SCALE {
                for dx in 0..SCALE {
                    let start = ((base_y + dy) * side + base_x + dx) * 4;
                    rgba[start..start + 4].copy_from_slice(&[0, 0, 0, 255]);
                }
            }
        }
    }
    let dimension = u32::try_from(side).map_err(|error| format!("QR image size: {error}"))?;
    let image = image::RgbaImage::from_raw(dimension, dimension, rgba)
        .ok_or_else(|| "QR buffer size mismatch".to_owned())?;
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|error| format!("encoding QR PNG: {error}"))?;
    Ok(png.into_inner())
}

/// Renders `text` onto an Android `Bitmap` with the platform's own
/// rasterizer and returns it as PNG bytes.
#[cfg(feature = "vision")]
fn text_png(env: &mut Env<'_>, text: &str) -> jni::errors::Result<Vec<u8>> {
    use jni::objects::{JByteArray, JValue};
    use jni::{jni_sig, jni_str};

    let argb = env
        .get_static_field(
            jni_str!("android/graphics/Bitmap$Config"),
            jni_str!("ARGB_8888"),
            jni_sig!("Landroid/graphics/Bitmap$Config;"),
        )?
        .l()?;
    let bitmap = env
        .call_static_method(
            jni_str!("android/graphics/Bitmap"),
            jni_str!("createBitmap"),
            jni_sig!("(IILandroid/graphics/Bitmap$Config;)Landroid/graphics/Bitmap;"),
            &[JValue::Int(640), JValue::Int(120), JValue::Object(&argb)],
        )?
        .l()?;
    let canvas = env.new_object(
        jni_str!("android/graphics/Canvas"),
        jni_sig!("(Landroid/graphics/Bitmap;)V"),
        &[JValue::Object(&bitmap)],
    )?;
    env.call_method(
        &canvas,
        jni_str!("drawColor"),
        jni_sig!("(I)V"),
        &[JValue::Int(-1)],
    )?;
    let paint = env.new_object(jni_str!("android/graphics/Paint"), jni_sig!("()V"), &[])?;
    env.call_method(
        &paint,
        jni_str!("setColor"),
        jni_sig!("(I)V"),
        &[JValue::Int(-0x0100_0000)],
    )?;
    env.call_method(
        &paint,
        jni_str!("setTextSize"),
        jni_sig!("(F)V"),
        &[JValue::Float(56.0)],
    )?;
    env.call_method(
        &paint,
        jni_str!("setAntiAlias"),
        jni_sig!("(Z)V"),
        &[JValue::Bool(true)],
    )?;
    let text = env.new_string(text)?;
    env.call_method(
        &canvas,
        jni_str!("drawText"),
        jni_sig!("(Ljava/lang/String;FFLandroid/graphics/Paint;)V"),
        &[
            JValue::Object(&text),
            JValue::Float(20.0),
            JValue::Float(84.0),
            JValue::Object(&paint),
        ],
    )?;
    let stream = env.new_object(
        jni_str!("java/io/ByteArrayOutputStream"),
        jni_sig!("()V"),
        &[],
    )?;
    let format = env
        .get_static_field(
            jni_str!("android/graphics/Bitmap$CompressFormat"),
            jni_str!("PNG"),
            jni_sig!("Landroid/graphics/Bitmap$CompressFormat;"),
        )?
        .l()?;
    env.call_method(
        &bitmap,
        jni_str!("compress"),
        jni_sig!("(Landroid/graphics/Bitmap$CompressFormat;ILjava/io/OutputStream;)Z"),
        &[
            JValue::Object(&format),
            JValue::Int(100),
            JValue::Object(&stream),
        ],
    )?;
    let bytes = env
        .call_method(&stream, jni_str!("toByteArray"), jni_sig!("()[B"), &[])?
        .l()?;
    env.convert_byte_array(env.cast_local::<JByteArray>(bytes)?)
}

/// The Google code scanner and the ML Kit document scanner need Play
/// services: `capabilities()` reports the device's own answer, and without
/// services `scan()` must fail with
/// [`waterkit_content::vision::VisionError::Unsupported`] instead of
/// presenting a UI or falling back to another realization.
#[cfg(feature = "vision")]
async fn record_android_vision(report: &mut TestReport) {
    let available = waterkit_content::vision::CodeScanner::capabilities().available;
    report.push(TestCase::passed_with_message(
        "vision.scanner_capabilities",
        format!("available={available}"),
    ));
    if available {
        report.push(TestCase::skipped(
            "vision.scanner_scan",
            "presenting the Google code scanner requires an interactive session",
        ));
    } else {
        match waterkit_content::vision::CodeScanner::new(waterkit_content::vision::Symbology::Qr)
            .scan()
            .await
        {
            Err(waterkit_content::vision::VisionError::Unsupported(message)) => {
                report.push(TestCase::passed_with_message(
                    "vision.scanner_scan",
                    format!("unsupported without Play services: {message}"),
                ));
            }
            other => report.push(TestCase::failed(
                "vision.scanner_scan",
                format!("scan() without Play services returned {other:?}"),
            )),
        }
    }

    let document_available = waterkit_content::vision::DocumentScanner::capabilities().available;
    report.push(TestCase::passed_with_message(
        "vision.document_scanner_capabilities",
        format!("available={document_available}"),
    ));
    if document_available {
        report.push(TestCase::skipped(
            "vision.document_scanner_scan",
            "presenting the ML Kit document scanner requires an interactive session",
        ));
    } else {
        match waterkit_content::vision::DocumentScanner::new()
            .scan()
            .await
        {
            Err(waterkit_content::vision::VisionError::Unsupported(message)) => {
                report.push(TestCase::passed_with_message(
                    "vision.document_scanner_scan",
                    format!("unsupported without Play services: {message}"),
                ));
            }
            other => report.push(TestCase::failed(
                "vision.document_scanner_scan",
                format!("scan() without Play services returned {other:?}"),
            )),
        }
    }
}
