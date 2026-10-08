//! `waterkit-test`, the runner that builds, launches and collects the
//! structured report of a `WaterKit` integration-test harness on macOS, iOS
//! and Android.

use clap::{Parser, Subcommand};
use eyre::{Context, Result};
use owo_colors::OwoColorize;
use process_control::{ChildExt, Control};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use toml_edit::DocumentMut;
use tracing::{info, warn};
use waterkit_test_report::{TestReport, from_json, parse_report_block};

mod android_device;
mod ios;

const MACOS_HEADERPAD_RUSTFLAGS: &str = "-C link-arg=-Wl,-headerpad_max_install_names";

/// Package name of the Android harness application.
const ANDROID_HARNESS_PACKAGE: &str = "com.waterkit.test";

/// How long the harness allows Android to bring the test activity to its first
/// frame, measured by `am start -W`.
///
/// A cold start right after `adb install` is the single most variable step in
/// the run: a physical Pixel 9 Pro draws the first frame in ~0.27s, while a
/// hosted-CI emulator sharing two vCPUs with the background dexopt that the
/// install just triggered has been measured at 55.2s. This bounds only the
/// launch, so a launch that never completes is reported as a launch failure
/// instead of silently consuming the budget meant for the test.
const ANDROID_LAUNCH_TIMEOUT: Duration = Duration::from_secs(300);

/// How long the native test itself has to run and write its report, measured
/// from the moment Android reports the activity displayed.
const ANDROID_REPORT_TIMEOUT: Duration = Duration::from_secs(60);

/// The report deadline when the harness is waiting for picker interaction.
const ANDROID_INTERACTIVE_REPORT_TIMEOUT: Duration = Duration::from_secs(600);

/// Cadence for polling the on-device report file over `adb`.
const ANDROID_REPORT_POLL_INTERVAL: Duration = Duration::from_millis(250);
const ANDROID_SMS_DELIVERY_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Deserialize)]
struct SmsRequest {
    sender: String,
    body: String,
}

#[derive(Parser)]
#[command(name = "waterkit-test")]
#[command(about = "CLI runner for WaterKit integration tests", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a crate on Android
    Android {
        /// Path to the crate to run
        crate_path: PathBuf,
        /// Enable cases that require interaction with Android pickers
        #[arg(long)]
        interactive: bool,
    },
    /// Run a crate on macOS
    Macos {
        /// Path to the crate to run
        crate_path: PathBuf,
    },
    /// Run a crate on the booted iOS simulator, or on a paired device
    Ios(ios::IosArgs),
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .without_time()
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Android {
            crate_path,
            interactive,
        } => run_android(&crate_path, interactive),
        Commands::Macos { crate_path } => run_macos(&crate_path),
        Commands::Ios(args) => ios::run(args),
    }
}

fn run_android(crate_path: &Path, interactive: bool) -> Result<()> {
    info!("{}", "Preparing Android test environment...".green().bold());

    let toolchain = AndroidToolchain::resolve()?;
    let feature = harness_feature(crate_path)?;
    let root_dir = workspace_root();
    let android_api = android_min_sdk(&root_dir)?;
    let sms_delivery = android_device_is_emulator(&toolchain)?;

    // Run cargo ndk build
    info!("{}", "Building Android test library...".yellow().bold());
    let mut args = vec![
        "ndk",
        "-t",
        "arm64-v8a",
        "-t",
        "x86_64",
        "-o",
        "tests/android/app/src/main/jniLibs",
        "-P",
        &android_api,
        "build",
        "-p",
        "waterkit-test-android",
    ];
    args.push("--features");
    args.push(feature);

    let status = std::process::Command::new("cargo")
        .current_dir(&root_dir)
        .args(&args)
        .status()
        .context("Failed to run cargo ndk")?;

    if !status.success() {
        eyre::bail!("Android build failed");
    }

    info!("{}", "Android libraries built successfully.".green().bold());

    build_android_apk(&root_dir, &toolchain, feature)?;
    install_android_apk(&root_dir, &toolchain)?;
    grant_android_permissions_for_feature(feature, &toolchain)?;
    // Wake the device only now: the build and install above take minutes, long
    // enough for the screen to time out again. The harness window keeps the
    // screen on from its first frame.
    android_device::wake_and_unlock(&toolchain)?;
    launch_android_test(&toolchain, sms_delivery, interactive)?;
    android_device::wait_for_harness_focus(&toolchain)?;
    let report_timeout = if interactive {
        ANDROID_INTERACTIVE_REPORT_TIMEOUT
    } else {
        ANDROID_REPORT_TIMEOUT
    };
    let report = wait_for_android_report(report_timeout, &toolchain, sms_delivery)?;
    ensure_report_success(&report)?;

    Ok(())
}

fn run_macos(crate_path: &Path) -> Result<()> {
    info!("{}", "Preparing macOS test environment...".green().bold());

    let crate_path = std::fs::canonicalize(crate_path).context("Failed to find crate path")?;
    let manifest_path = crate_path.join("Cargo.toml");
    if !manifest_path.exists() {
        eyre::bail!("No Cargo.toml found at {}", crate_path.display());
    }

    let root_dir = workspace_root();
    let metadata = parse_macos_metadata(&manifest_path)?;
    info!("Target crate: {}", crate_path.display());
    info!("Package: {}", metadata.package_name);
    info!("Primary binary: {}", metadata.bin_name);

    info!("{}", "Building macOS test binary...".yellow().bold());
    let mut build_command = std::process::Command::new("cargo");
    build_command
        .current_dir(&root_dir)
        .args(["build", "--manifest-path"])
        .arg(&manifest_path);
    install_macos_headerpad_rustflags(&mut build_command);
    let build_status = build_command
        .status()
        .context("Failed to run cargo build for macOS test crate")?;
    if !build_status.success() {
        eyre::bail!("macOS build failed for {}", metadata.package_name);
    }

    let binary_path = root_dir.join("target/debug").join(&metadata.bin_name);
    if !binary_path.exists() {
        eyre::bail!(
            "Built binary not found at {}. Ensure crate has a runnable binary target.",
            binary_path.display()
        );
    }

    let info_plist_path = crate_path.join("Info.plist");
    let log_path = root_dir
        .join("target/debug")
        .join(format!("{}.log", metadata.bin_name));
    if log_path.exists() {
        std::fs::remove_file(&log_path)
            .with_context(|| format!("Failed to remove stale {}", log_path.display()))?;
    }

    let output = if info_plist_path.exists() {
        run_macos_app_bundle(
            &root_dir,
            &metadata.bin_name,
            &binary_path,
            &info_plist_path,
        )?;
        std::fs::read_to_string(&log_path)
            .with_context(|| format!("macOS app did not write {}", log_path.display()))?
    } else {
        run_macos_cli_binary(&root_dir, &binary_path, &metadata.bin_name)?
    };

    let report = parse_process_report("macOS", &metadata.package_name, &output)?;
    ensure_report_success(&report)?;

    Ok(())
}

#[derive(Debug)]
struct MacosMetadata {
    package_name: String,
    bin_name: String,
}

fn parse_macos_metadata(manifest_path: &Path) -> Result<MacosMetadata> {
    let manifest_text = std::fs::read_to_string(manifest_path)
        .with_context(|| format!("Read {}", manifest_path.display()))?;
    let manifest = manifest_text
        .parse::<DocumentMut>()
        .with_context(|| format!("Parse {}", manifest_path.display()))?;

    let package_name = manifest
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(|name| name.as_str())
        .ok_or_else(|| eyre::eyre!("Missing package.name in {}", manifest_path.display()))?
        .to_owned();

    let bin_name = select_primary_bin_name(&manifest, &package_name)?;

    Ok(MacosMetadata {
        package_name,
        bin_name,
    })
}

fn select_primary_bin_name(manifest: &DocumentMut, package_name: &str) -> Result<String> {
    // Indexing a missing key panics on toml_edit documents; `get` keeps
    // crates that rely on the default `src/main.rs` binary working.
    let Some(bin_tables) = manifest.get("bin").and_then(|bin| bin.as_array_of_tables()) else {
        return Ok(package_name.to_owned());
    };

    if bin_tables.is_empty() {
        return Ok(package_name.to_owned());
    }

    if bin_tables.len() > 1 {
        warn!(
            "Multiple [[bin]] targets found; defaulting to the first one for generic macOS runner"
        );
    }

    let first = bin_tables
        .iter()
        .next()
        .ok_or_else(|| eyre::eyre!("First [[bin]] entry is missing"))?;
    let name = first["name"]
        .as_str()
        .ok_or_else(|| eyre::eyre!("First [[bin]] entry is missing a name field"))?;
    Ok(name.to_owned())
}

fn run_macos_cli_binary(root_dir: &Path, binary_path: &Path, bin_name: &str) -> Result<String> {
    info!(
        "{}",
        "No Info.plist found; running binary directly."
            .yellow()
            .bold()
    );

    let output = std::process::Command::new(binary_path)
        .current_dir(root_dir)
        .output()
        .with_context(|| format!("Failed to run macOS test binary {bin_name}"))?;

    if !output.status.success() {
        eyre::bail!("macOS CLI run failed for binary {}", bin_name);
    }

    Ok(output_text(&output))
}

fn run_macos_app_bundle(
    root_dir: &Path,
    bin_name: &str,
    built_binary: &Path,
    info_plist_path: &Path,
) -> Result<()> {
    info!(
        "{}",
        "Info.plist detected; creating, signing, and launching .app bundle."
            .yellow()
            .bold()
    );

    let app_dir = root_dir
        .join("target/debug")
        .join(format!("{bin_name}.app"));
    let contents_dir = app_dir.join("Contents");
    let macos_dir = contents_dir.join("MacOS");
    let app_binary = macos_dir.join(bin_name);

    if app_dir.exists() {
        std::fs::remove_dir_all(&app_dir)
            .with_context(|| format!("Failed to remove {}", app_dir.display()))?;
    }
    std::fs::create_dir_all(&macos_dir)
        .with_context(|| format!("Failed to create {}", macos_dir.display()))?;

    std::fs::copy(built_binary, &app_binary).with_context(|| {
        format!(
            "Failed to copy built binary from {} to {}",
            built_binary.display(),
            app_binary.display()
        )
    })?;
    std::fs::copy(info_plist_path, contents_dir.join("Info.plist")).with_context(|| {
        format!(
            "Failed to copy Info.plist from {}",
            info_plist_path.display()
        )
    })?;

    add_swift_rpath_if_exists(&app_binary, Path::new("/usr/lib/swift"))?;

    let xcode_path_output = std::process::Command::new("xcode-select")
        .args(["-p"])
        .output()
        .context("Failed to query xcode-select -p")?;
    if xcode_path_output.status.success() {
        let xcode_path = String::from_utf8(xcode_path_output.stdout)
            .context("xcode-select output is not valid UTF-8")?
            .trim()
            .to_owned();
        let xcode_swift_lib = PathBuf::from(xcode_path)
            .join("Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx");
        add_swift_rpath_if_exists(&app_binary, &xcode_swift_lib)?;
    }

    let codesign_status = std::process::Command::new("codesign")
        .args(["--force", "--sign", "-"])
        .arg(&app_dir)
        .status()
        .context("Failed to run codesign for macOS bundle")?;
    if !codesign_status.success() {
        eyre::bail!("codesign failed for {}", app_dir.display());
    }

    // `open -W` waits by attaching a kqueue to the launched process and fails
    // with "initial call to kevent() failed: No such process" whenever the app
    // finishes before `open` can attach — exactly what a fast
    // permission-skipped run does. Spawning the bundle executable directly
    // hands us the pid, so `wait` observes the termination no matter how
    // quickly it happens; the bundle is still resolved from the executable
    // path, so TCC attribution is unchanged.
    info!("{}", "Launching app bundle...".green().bold());
    let run_status = std::process::Command::new(&app_binary)
        .status()
        .with_context(|| format!("Failed to launch {}", app_binary.display()))?;
    if !run_status.success() {
        // The structured report in the log file is the result contract; a
        // non-zero exit only adds context when the report never arrived.
        warn!("{} exited with {run_status}", app_binary.display());
    }

    Ok(())
}

struct AndroidToolchain {
    sdk_root: PathBuf,
    adb: PathBuf,
}

fn android_min_sdk(root_dir: &Path) -> Result<String> {
    let build_gradle = root_dir.join("tests/android/app/build.gradle.kts");
    let contents = std::fs::read_to_string(&build_gradle)
        .with_context(|| format!("Failed to read {}", build_gradle.display()))?;
    parse_android_min_sdk(&contents).ok_or_else(|| {
        eyre::eyre!(
            "Could not find defaultConfig minSdk in {}",
            build_gradle.display()
        )
    })
}

fn parse_android_min_sdk(build_gradle: &str) -> Option<String> {
    let default_config = regex::Regex::new(r"(?s)defaultConfig\s*\{(?<body>.*?)\n\s*\}").ok()?;
    let min_sdk = regex::Regex::new(r"(?m)^\s*minSdk\s*=\s*(?<api>\d+)\s*$").ok()?;
    let body = default_config
        .captures(build_gradle)?
        .name("body")?
        .as_str();
    Some(min_sdk.captures(body)?.name("api")?.as_str().to_owned())
}

impl AndroidToolchain {
    fn resolve() -> Result<Self> {
        let adb = which::which("adb").context("Android platform tool `adb` was not found")?;
        let adb = std::fs::canonicalize(&adb)
            .with_context(|| format!("Failed to resolve adb path {}", adb.display()))?;
        let sdk_root = configured_android_sdk_root()
            .or_else(|| sdk_root_from_adb(&adb))
            .ok_or_else(|| {
                eyre::eyre!(
                    "Could not determine the Android SDK root from ANDROID_SDK_ROOT, ANDROID_HOME, or adb path {}",
                    adb.display()
                )
            })?;

        if !sdk_root.join("platforms").is_dir() {
            eyre::bail!(
                "Android SDK root {} does not contain a platforms directory",
                sdk_root.display()
            );
        }

        Ok(Self { sdk_root, adb })
    }
}

fn configured_android_sdk_root() -> Option<PathBuf> {
    ["ANDROID_SDK_ROOT", "ANDROID_HOME"]
        .into_iter()
        .find_map(std::env::var_os)
        .map(PathBuf::from)
}

fn sdk_root_from_adb(adb: &Path) -> Option<PathBuf> {
    let platform_tools = adb.parent()?;
    if platform_tools.file_name()? != "platform-tools" {
        return None;
    }
    platform_tools.parent().map(Path::to_path_buf)
}

fn build_android_apk(root_dir: &Path, toolchain: &AndroidToolchain, feature: &str) -> Result<()> {
    info!("{}", "Building Android APK...".yellow().bold());
    let android_dir = root_dir.join("tests/android");
    let gradlew = android_dir.join("gradlew");
    let status = std::process::Command::new(&gradlew)
        .current_dir(&android_dir)
        .env("ANDROID_HOME", &toolchain.sdk_root)
        .env("ANDROID_SDK_ROOT", &toolchain.sdk_root)
        .arg(":app:assembleDebug")
        // The app module resolves each crate's
        // [package.metadata.waterui.android] declarations through cargo
        // metadata for exactly this feature set (the same channel the water
        // CLI's classpath staging consumes).
        .arg(format!("-PwaterkitFeatures={feature}"))
        .status()
        .context("Failed to run Android Gradle build")?;

    if !status.success() {
        eyre::bail!("Android APK build failed");
    }

    Ok(())
}

fn install_android_apk(root_dir: &Path, toolchain: &AndroidToolchain) -> Result<()> {
    info!("{}", "Installing Android APK...".yellow().bold());
    let apk = root_dir.join("tests/android/app/build/outputs/apk/debug/app-debug.apk");
    if !apk.exists() {
        eyre::bail!("Android APK not found at {}", apk.display());
    }

    let status = std::process::Command::new(&toolchain.adb)
        .arg("install")
        .arg("-r")
        .arg(&apk)
        .status()
        .context("Failed to install Android APK with adb")?;

    if !status.success() {
        eyre::bail!("Android APK installation failed");
    }

    Ok(())
}

fn grant_android_permissions_for_feature(
    feature: &str,
    toolchain: &AndroidToolchain,
) -> Result<()> {
    let permissions: &[&str] = match feature {
        "full" => &[
            "android.permission.ACCESS_FINE_LOCATION",
            "android.permission.ACCESS_COARSE_LOCATION",
            "android.permission.CAMERA",
            "android.permission.RECORD_AUDIO",
            "android.permission.READ_CONTACTS",
            "android.permission.READ_CALENDAR",
            "android.permission.POST_NOTIFICATIONS",
        ],
        "location" | "permission" => &[
            "android.permission.ACCESS_FINE_LOCATION",
            "android.permission.ACCESS_COARSE_LOCATION",
        ],
        "camera" | "vision" => &["android.permission.CAMERA"],
        "audio" | "speech" => &["android.permission.RECORD_AUDIO"],
        "contacts" => &["android.permission.READ_CONTACTS"],
        "calendar" => &["android.permission.READ_CALENDAR"],
        "notification" => &["android.permission.POST_NOTIFICATIONS"],
        _ => &[],
    };

    for permission in permissions {
        run_adb(
            toolchain,
            ["shell", "pm", "grant", ANDROID_HARNESS_PACKAGE, permission],
        )?;
    }

    Ok(())
}

fn launch_android_test(
    toolchain: &AndroidToolchain,
    sms_delivery: bool,
    interactive: bool,
) -> Result<()> {
    run_adb(
        toolchain,
        ["shell", "am", "force-stop", ANDROID_HARNESS_PACKAGE],
    )?;
    run_adb(
        toolchain,
        [
            "shell",
            "run-as",
            ANDROID_HARNESS_PACKAGE,
            "rm",
            "-f",
            "files/waterkit-test-report.json",
            "files/waterkit-sms-request.json",
            "files/waterkit-sms-request.json.tmp",
        ],
    )?;
    // Everything logcat holds from here on belongs to this run, so a failure
    // dump can be complete instead of an arbitrary tail that a busy device
    // fills with unrelated system chatter in seconds.
    run_adb(toolchain, ["logcat", "-c"])?;

    // `-W` makes Android tell us when the activity actually reached its first
    // frame instead of returning as soon as the start request is queued. The
    // report deadline then bounds the native test alone, which is the thing it
    // is meant to bound.
    let component = format!("{ANDROID_HARNESS_PACKAGE}/.MainActivity");
    let mut args = vec!["shell", "am", "start", "-W", "-n", component.as_str()];
    if sms_delivery {
        args.extend_from_slice(&["--ez", "sms_delivery", "true"]);
    }
    args.extend_from_slice(&[
        "--ez",
        "run_test",
        "true",
        "--ez",
        "interactive",
        if interactive { "true" } else { "false" },
    ]);
    let output = run_adb_with_timeout(
        toolchain,
        &args,
        ANDROID_LAUNCH_TIMEOUT,
        "launch the Android test activity",
    )?;

    let launch =
        String::from_utf8(output.stdout).context("`am start -W` output was not valid UTF-8")?;
    let status = am_start_field(&launch, "Status").unwrap_or("<missing>");
    if status != "ok" {
        eyre::bail!(
            "Android test activity did not launch (Status: {status}).\n\
             --- am start -W output ---\n{}\n--- adb stderr ---\n{}",
            launch.trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    info!(
        "Android test activity displayed in {}ms (LaunchState: {})",
        am_start_field(&launch, "TotalTime").unwrap_or("<unknown>"),
        am_start_field(&launch, "LaunchState").unwrap_or("<unknown>"),
    );

    Ok(())
}

/// Reads one `Key: value` line out of `am start -W` output.
fn am_start_field<'a>(output: &'a str, key: &str) -> Option<&'a str> {
    output.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name.trim() == key).then(|| value.trim())
    })
}

/// Runs an `adb` command that must not hang, capturing its output.
fn run_adb_with_timeout(
    toolchain: &AndroidToolchain,
    args: &[&str],
    timeout: Duration,
    description: &str,
) -> Result<Output> {
    let mut command = Command::new(&toolchain.adb);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    run_with_timeout(command, timeout, description)
}

fn wait_for_android_report(
    timeout: Duration,
    toolchain: &AndroidToolchain,
    sms_delivery: bool,
) -> Result<TestReport> {
    let deadline = Instant::now() + timeout;

    loop {
        if sms_delivery && let Some(request) = poll_sms_request(toolchain)? {
            deliver_sms_request(toolchain, &request)?;
        }

        let output = std::process::Command::new(&toolchain.adb)
            .args([
                "exec-out",
                "run-as",
                ANDROID_HARNESS_PACKAGE,
                "sh",
                "-c",
                "test -s files/waterkit-test-report.json && cat files/waterkit-test-report.json",
            ])
            .output()
            .context("Failed to read Android test report with adb")?;

        if output.status.success() && !output.stdout.is_empty() {
            let json = String::from_utf8(output.stdout)
                .context("Android test report was not valid UTF-8")?;
            return from_json(&json).context("Failed to parse Android test report JSON");
        }

        if Instant::now() >= deadline {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // The poll above is silent by design: `test -s` just exits non-zero
            // while the report is absent, so on a timeout its stderr says
            // nothing about why. Everything that would explain it — a crash
            // after the first frame, a panic inside the harness activity, a
            // sensor the device does not provide — is in logcat, which was
            // cleared just before launch and may be about to be destroyed along
            // with the device. Take all of it with us.
            let logcat = std::process::Command::new(&toolchain.adb)
                .args(["logcat", "-d"])
                .output()
                .map_or_else(
                    |error| format!("<could not read logcat: {error}>"),
                    |logcat| String::from_utf8_lossy(&logcat.stdout).trim().to_string(),
                );
            eyre::bail!(
                "The Android test activity was displayed but no test report appeared within {timeout:?}.\n\
                 last adb stderr: {}\n\
                 --- logcat since launch ---\n{logcat}",
                stderr.trim()
            );
        }

        thread::sleep(ANDROID_REPORT_POLL_INTERVAL);
    }
}

fn android_device_is_emulator(toolchain: &AndroidToolchain) -> Result<bool> {
    let output = run_adb_with_timeout(
        toolchain,
        &["shell", "getprop", "ro.boot.qemu"],
        Duration::from_secs(10),
        "query Android emulator status",
    )?;
    if !output.status.success() {
        eyre::bail!(
            "Could not query ro.boot.qemu: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let property = String::from_utf8(output.stdout)
        .context("Android emulator property was not valid UTF-8")?;
    Ok(property.trim() == "1")
}

fn poll_sms_request(toolchain: &AndroidToolchain) -> Result<Option<SmsRequest>> {
    let output = std::process::Command::new(&toolchain.adb)
        .args([
            "exec-out",
            "run-as",
            ANDROID_HARNESS_PACKAGE,
            "sh",
            "-c",
            "test -s files/waterkit-sms-request.json && cat files/waterkit-sms-request.json",
        ])
        .output()
        .context("Failed to read Android SMS request with adb")?;
    if !output.status.success() || output.stdout.is_empty() {
        return Ok(None);
    }

    let request: SmsRequest = serde_json::from_slice(&output.stdout)
        .context("Failed to parse Android SMS request JSON")?;
    run_adb(
        toolchain,
        [
            "shell",
            "run-as",
            ANDROID_HARNESS_PACKAGE,
            "rm",
            "-f",
            "files/waterkit-sms-request.json",
        ],
    )?;
    Ok(Some(request))
}

fn deliver_sms_request(toolchain: &AndroidToolchain, request: &SmsRequest) -> Result<()> {
    let args = [
        "emu",
        "sms",
        "send",
        request.sender.as_str(),
        request.body.as_str(),
    ];
    let output = run_adb_with_timeout(
        toolchain,
        &args,
        ANDROID_SMS_DELIVERY_TIMEOUT,
        "deliver an OTP SMS to the Android emulator",
    )?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() || stdout.contains("KO:") {
        eyre::bail!(
            "adb emu sms send failed: {}\n{}",
            stdout.trim(),
            stderr.trim()
        );
    }
    info!("Delivered a test OTP SMS to sender {}", request.sender);
    Ok(())
}

fn run_adb<const N: usize>(toolchain: &AndroidToolchain, args: [&str; N]) -> Result<()> {
    let status = std::process::Command::new(&toolchain.adb)
        .args(args)
        .status()
        .context("Failed to run adb")?;

    if !status.success() {
        eyre::bail!("adb command failed");
    }

    Ok(())
}

/// Resolves the harness feature that exercises the crate at `crate_path`.
fn harness_feature(crate_path: &Path) -> Result<&'static str> {
    let crate_path = std::fs::canonicalize(crate_path).context("Failed to find crate path")?;
    let manifest_path = crate_path.join("Cargo.toml");
    if !manifest_path.exists() {
        eyre::bail!("No Cargo.toml found at {}", crate_path.display());
    }
    info!("Target crate: {}", crate_path.display());

    let manifest = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("Read {}", manifest_path.display()))?
        .parse::<DocumentMut>()
        .with_context(|| format!("Parse {}", manifest_path.display()))?;
    let package_name = manifest
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(|name| name.as_str())
        .ok_or_else(|| eyre::eyre!("Missing package.name in {}", manifest_path.display()))?;
    get_crate_feature(package_name).ok_or_else(|| {
        eyre::eyre!("Unsupported crate package name for harness features: {package_name}")
    })
}

/// Runs `command` to completion, killing it when it outlives `timeout`.
///
/// Its piped stdout and stderr are drained while it runs: a child whose output
/// outgrows the pipe buffer would otherwise block on the write and be killed at
/// the deadline with its work already done.
fn run_with_timeout(mut command: Command, timeout: Duration, description: &str) -> Result<Output> {
    let output = command
        .spawn()
        .with_context(|| format!("Failed to start the command that should {description}"))?
        .controlled_with_output()
        .time_limit(timeout)
        .terminate_for_timeout()
        .wait()
        .with_context(|| format!("Failed to wait for the command that should {description}"))?
        .ok_or_else(|| eyre::eyre!("Did not {description} within {timeout:?}"))?;
    Ok(output.into_std_lossy())
}

fn parse_process_report(platform: &str, package_name: &str, output: &str) -> Result<TestReport> {
    let report = parse_report_block(output)
        .context("Failed to parse structured test report")?
        .ok_or_else(|| {
            eyre::eyre!("{platform} test {package_name} did not emit a structured test report")
        })?;

    if report.cases.is_empty() {
        eyre::bail!("{platform} test {package_name} emitted an empty report");
    }

    Ok(report)
}

fn ensure_report_success(report: &TestReport) -> Result<()> {
    info!(
        "Structured report: platform={} crate={} passed={} skipped={} failed={}",
        report.platform,
        report.crate_name,
        report.passed_count(),
        report.skipped_count(),
        report.failed_count()
    );

    for case in &report.cases {
        if let Some(message) = &case.message {
            info!("  {:?} {}: {message}", case.status, case.name);
        } else {
            info!("  {:?} {}", case.status, case.name);
        }
    }

    if report.has_failures() {
        eyre::bail!("WaterKit test failures: {}", report.failure_summary());
    }

    Ok(())
}

fn output_text(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("{stdout}\n{stderr}")
}

fn install_macos_headerpad_rustflags(command: &mut std::process::Command) {
    command.env("RUSTFLAGS", macos_headerpad_rustflags());
}

fn macos_headerpad_rustflags() -> String {
    match std::env::var("RUSTFLAGS") {
        Ok(flags) if flags.contains("headerpad_max_install_names") => flags,
        Ok(flags) if flags.trim().is_empty() => MACOS_HEADERPAD_RUSTFLAGS.to_owned(),
        Ok(flags) => format!("{flags} {MACOS_HEADERPAD_RUSTFLAGS}"),
        Err(_) => MACOS_HEADERPAD_RUSTFLAGS.to_owned(),
    }
}

fn add_swift_rpath_if_exists(binary_path: &Path, rpath: &Path) -> Result<()> {
    if !rpath.exists() {
        return Ok(());
    }

    let output = std::process::Command::new("install_name_tool")
        .args(["-add_rpath"])
        .arg(rpath)
        .arg(binary_path)
        .output()
        .with_context(|| {
            format!(
                "Failed to run install_name_tool for {}",
                binary_path.display()
            )
        })?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("would duplicate path") || stderr.contains("already exists in") {
        return Ok(());
    }

    eyre::bail!(
        "install_name_tool failed for {} with rpath {}: {}",
        binary_path.display(),
        rpath.display(),
        stderr.trim()
    );
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn get_crate_feature(package_name: &str) -> Option<&'static str> {
    if package_name == "waterkit" {
        Some("full")
    } else if package_name.contains("sensor") {
        Some("sensor")
    } else if package_name.contains("biometric") {
        Some("biometric")
    } else if package_name.contains("location") {
        Some("location")
    } else if package_name.contains("audio") {
        Some("audio")
    } else if package_name.contains("camera") {
        Some("camera")
    } else if package_name.contains("vision") {
        Some("vision")
    } else if package_name.contains("clipboard") {
        Some("clipboard")
    } else if package_name.contains("codec") {
        Some("codec")
    } else if package_name.contains("dialog") {
        Some("dialog")
    } else if package_name.contains("fs") {
        Some("fs")
    } else if package_name.contains("haptic") {
        Some("haptic")
    } else if package_name.contains("notification") {
        Some("notification")
    } else if package_name.contains("permission") {
        Some("permission")
    } else if package_name.contains("secret") {
        Some("secret")
    } else if package_name.contains("system") {
        Some("system")
    } else if package_name.contains("video") {
        Some("video")
    } else if package_name.contains("bluetooth") {
        Some("bluetooth")
    } else if package_name.contains("nfc") {
        Some("nfc")
    } else if package_name.contains("share") {
        Some("share")
    } else if package_name.contains("speech") {
        Some("speech")
    } else if package_name.contains("contacts") {
        Some("contacts")
    } else if package_name.contains("calendar") {
        Some("calendar")
    } else if package_name.contains("health") {
        Some("health")
    } else if package_name.contains("deeplink") {
        Some("deeplink")
    } else if package_name.contains("screen") {
        Some("screen")
    } else if package_name.contains("background") {
        Some("background")
    } else if package_name.contains("passkey") {
        Some("passkey")
    } else if package_name.contains("otp") {
        Some("otp")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{am_start_field, get_crate_feature, parse_android_min_sdk, sdk_root_from_adb};
    use std::path::{Path, PathBuf};

    /// A real `am start -W` reply, captured from a Pixel 9 Pro.
    const AM_START_OK: &str = include_str!("../tests/fixtures/am-start-w.txt");

    #[test]
    fn reads_am_start_launch_fields() {
        assert_eq!(am_start_field(AM_START_OK, "Status"), Some("ok"));
        assert_eq!(am_start_field(AM_START_OK, "LaunchState"), Some("COLD"));
        assert_eq!(am_start_field(AM_START_OK, "TotalTime"), Some("271"));
    }

    #[test]
    fn reports_missing_am_start_field() {
        assert_eq!(am_start_field(AM_START_OK, "Error"), None);
    }

    #[test]
    fn derives_android_sdk_root_from_platform_tools_adb() {
        assert_eq!(
            sdk_root_from_adb(Path::new("/opt/android-sdk/platform-tools/adb")),
            Some(PathBuf::from("/opt/android-sdk"))
        );
    }

    #[test]
    fn rejects_adb_outside_android_platform_tools() {
        assert_eq!(sdk_root_from_adb(Path::new("/usr/local/bin/adb")), None);
    }

    #[test]
    fn selects_full_harness_for_waterkit_facade() {
        assert_eq!(get_crate_feature("waterkit"), Some("full"));
    }

    #[test]
    fn selects_otp_harness_feature() {
        assert_eq!(get_crate_feature("waterkit-otp"), Some("otp"));
    }

    #[test]
    fn parses_android_min_sdk_from_default_config() {
        let build_gradle = include_str!("../tests/fixtures/android-build.gradle.kts");
        assert_eq!(parse_android_min_sdk(build_gradle).as_deref(), Some("26"));
    }
}
