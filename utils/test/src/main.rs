use clap::{Parser, Subcommand};
use eyre::{Context, Result};
use owo_colors::OwoColorize;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::thread;
use std::time::{Duration, Instant};
use toml_edit::DocumentMut;
use tracing::{info, warn};
use waterkit_test_report::{TestReport, from_json, parse_report_block};

const MACOS_HEADERPAD_RUSTFLAGS: &str = "-C link-arg=-Wl,-headerpad_max_install_names";

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
    },
    /// Run a crate on macOS
    Macos {
        /// Path to the crate to run
        crate_path: PathBuf,
    },
    /// Run a crate on iOS
    Ios {
        /// Path to the crate to run
        crate_path: PathBuf,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .without_time()
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Android { crate_path } => run_android(&crate_path),
        Commands::Macos { crate_path } => run_macos(&crate_path),
        Commands::Ios { crate_path } => run_ios(&crate_path),
    }
}

fn run_android(crate_path: &Path) -> Result<()> {
    info!("{}", "Preparing Android test environment...".green().bold());

    let toolchain = AndroidToolchain::resolve()?;
    with_android_device_awake(&toolchain, || run_android_awake(crate_path, &toolchain))
}

fn run_android_awake(crate_path: &Path, toolchain: &AndroidToolchain) -> Result<()> {
    // 1. Verify crate path
    let crate_path = std::fs::canonicalize(crate_path).context("Failed to find crate path")?;

    if !crate_path.join("Cargo.toml").exists() {
        eyre::bail!("No Cargo.toml found at {}", crate_path.display());
    }

    info!("Target crate: {}", crate_path.display());

    // 2. Resolve workspace root
    let root_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap() // tools
        .parent()
        .unwrap() // kit (root)
        .to_path_buf();
    let android_api = android_min_sdk(&root_dir)?;

    // 3. Get feature
    let content_cargo_path = crate_path.join("Cargo.toml");
    let content_toml_str =
        std::fs::read_to_string(&content_cargo_path).context("Read content toml")?;
    let content_doc = content_toml_str
        .parse::<DocumentMut>()
        .context("Parse content toml")?;
    let package_name = content_doc["package"]["name"].as_str().unwrap_or("");
    let feature = get_crate_feature(package_name).ok_or_else(|| {
        eyre::eyre!("Unsupported crate package name for harness features: {package_name}")
    })?;

    // 4. Run cargo ndk build
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

    build_android_apk(&root_dir, toolchain)?;
    install_android_apk(&root_dir, toolchain)?;
    grant_android_permissions_for_feature(feature, toolchain)?;
    launch_android_test(toolchain)?;
    let report = wait_for_android_report(Duration::from_secs(60), toolchain)?;
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

fn run_ios(crate_path: &Path) -> Result<()> {
    info!("{}", "Preparing iOS test environment...".green().bold());

    // 1. Verify crate path
    let crate_path = std::fs::canonicalize(crate_path).context("Failed to find crate path")?;

    if !crate_path.join("Cargo.toml").exists() {
        eyre::bail!("No Cargo.toml found at {}", crate_path.display());
    }

    info!("Target crate: {}", crate_path.display());

    // 2. Resolve workspace root
    let root_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap() // tools
        .parent()
        .unwrap() // kit (root)
        .to_path_buf();

    // 2.5 Get feature
    let content_cargo_path = crate_path.join("Cargo.toml");
    let content_toml_str =
        std::fs::read_to_string(&content_cargo_path).context("Read content toml")?;
    let content_doc = content_toml_str
        .parse::<DocumentMut>()
        .context("Parse content toml")?;
    let package_name = content_doc["package"]["name"].as_str().unwrap_or("");
    let feature = get_crate_feature(package_name).ok_or_else(|| {
        eyre::eyre!("Unsupported crate package name for harness features: {package_name}")
    })?;

    // 3. Build for iOS Simulator
    info!("{}", "Building iOS test library...".yellow().bold());
    let mut args = vec![
        "build",
        "--target",
        "aarch64-apple-ios-sim",
        "-p",
        "waterkit-test-ios",
    ];
    args.push("--features");
    args.push(feature);

    let status = std::process::Command::new("cargo")
        .current_dir(&root_dir)
        .args(&args)
        .status()
        .context("Failed to run cargo build")?;

    if !status.success() {
        eyre::bail!("iOS build failed");
    }

    // 4. Swift Compile
    info!("{}", "Compiling Swift app...".yellow().bold());

    // Scan for extra .swift sources in the target crate
    let mut extra_swift_sources = Vec::new();
    let sys_apple_dir = crate_path.join("src/sys/apple");
    #[allow(clippy::collapsible_if)]
    if sys_apple_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(sys_apple_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "swift") {
                    info!("Found extra Swift source: {}", path.display());
                    extra_swift_sources.push(path);
                }
            }
        }
    }

    // Ensure Generated directory exists (usually done by build script, but ensure path logic is sound)

    // 4.1 Get SDK Path
    let sdk_path_output = std::process::Command::new("xcrun")
        .args(["--sdk", "iphonesimulator", "--show-sdk-path"])
        .output()
        .context("Failed to get SDK path")?;
    let sdk_path = String::from_utf8(sdk_path_output.stdout)?
        .trim()
        .to_string();

    let mut swiftc_cmd = std::process::Command::new("xcrun");
    swiftc_cmd
        .current_dir(&root_dir)
        .arg("swiftc")
        .arg("-target")
        .arg("arm64-apple-ios17.0-simulator") // Target iOS 17 (Sim)
        .arg("-sdk")
        .arg(&sdk_path)
        .arg("-I")
        .arg("tests/ios/app/WaterKitTest/Generated")
        .arg("-import-objc-header")
        .arg("tests/ios/app/WaterKitTest/Generated/Bridging-Header.h")
        .arg("-L")
        .arg("target/aarch64-apple-ios-sim/debug")
        .arg("-lwaterkit_test_ios")
        .arg("-framework")
        .arg("CoreFoundation")
        .arg("-framework")
        .arg("Security")
        .arg("-framework")
        .arg("Foundation")
        .arg("-framework")
        .arg("SwiftUI")
        .arg("tests/ios/app/WaterKitTest/WaterKitTestApp.swift")
        .arg("tests/ios/app/WaterKitTest/ContentView.swift")
        .arg("tests/ios/app/WaterKitTest/Generated/SwiftBridgeCore.swift")
        .arg("tests/ios/app/WaterKitTest/Generated/waterkit-test-ios/waterkit-test-ios.swift");

    // Add extra sources
    for src in extra_swift_sources {
        swiftc_cmd.arg(src);
    }

    let status = swiftc_cmd
        .arg("-o")
        .arg("WaterKitTestBinary")
        .status()
        .context("Failed to compile Swift app")?;

    if !status.success() {
        eyre::bail!("Swift compilation failed");
    }

    // 5. Bundle
    info!("{}", "Bundling app...".yellow().bold());
    let app_dir = root_dir.join("WaterKitTest.app");
    if app_dir.exists() {
        std::fs::remove_dir_all(&app_dir)?;
    }
    std::fs::create_dir_all(&app_dir)?;

    std::fs::rename(
        root_dir.join("WaterKitTestBinary"),
        app_dir.join("WaterKitTest"),
    )?;

    std::fs::copy(
        root_dir.join("tests/ios/app/Info.plist"),
        app_dir.join("Info.plist"),
    )?;

    // 6. Codesign
    info!("{}", "Codesigning...".yellow().bold());
    let status = std::process::Command::new("codesign")
        .args(["-s", "-", "WaterKitTest.app"])
        .current_dir(&root_dir)
        .status()
        .context("Failed to codesign")?;

    if !status.success() {
        eyre::bail!("Codesign failed");
    }

    // 7. Install & Launch
    info!("{}", "Installing to Simulator (booted)...".yellow().bold());
    let simulator_id = "booted"; // Use "booted" to target the active simulator automatically!

    let status = std::process::Command::new("xcrun")
        .args(["simctl", "install", simulator_id, "WaterKitTest.app"])
        .current_dir(&root_dir)
        .status()
        .context("Failed to install to simulator")?;

    if !status.success() {
        eyre::bail!("Installation failed (ensure a simulator is booted)");
    }

    let report_path = ios_report_path(simulator_id)?;
    if report_path.exists() {
        std::fs::remove_file(&report_path)
            .with_context(|| format!("Failed to remove stale {}", report_path.display()))?;
    }

    info!("{}", "Launching app...".green().bold());
    let status = std::process::Command::new("xcrun")
        .args([
            "simctl",
            "launch",
            "--console",
            simulator_id,
            "com.waterkit.test",
            "--waterkit-run-test",
        ])
        .current_dir(&root_dir)
        .status()
        .context("Failed to launch app")?;

    if !status.success() {
        eyre::bail!("Launch failed");
    }

    let report_json = std::fs::read_to_string(&report_path)
        .with_context(|| format!("iOS app did not write {}", report_path.display()))?;
    let report = from_json(&report_json)
        .with_context(|| format!("Failed to parse {}", report_path.display()))?;
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

    let package_name = manifest["package"]["name"]
        .as_str()
        .ok_or_else(|| eyre::eyre!("Missing package.name in {}", manifest_path.display()))?
        .to_owned();

    let bin_name = select_primary_bin_name(&manifest, &package_name)?;

    Ok(MacosMetadata {
        package_name,
        bin_name,
    })
}

fn select_primary_bin_name(manifest: &DocumentMut, package_name: &str) -> Result<String> {
    let Some(bin_tables) = manifest["bin"].as_array_of_tables() else {
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

    info!("{}", "Launching app bundle...".green().bold());
    let open_status = std::process::Command::new("open")
        .arg("-W")
        .arg(&app_dir)
        .status()
        .context("Failed to run open -W for macOS app bundle")?;
    if !open_status.success() {
        eyre::bail!("open -W failed for {}", app_dir.display());
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

fn with_android_device_awake<T>(
    toolchain: &AndroidToolchain,
    run: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let original = android_stay_awake_setting(toolchain)?;
    set_android_stay_awake(toolchain, original | 2)?;

    let run_result =
        run_adb(toolchain, ["shell", "input", "keyevent", "KEYCODE_WAKEUP"]).and_then(|()| run());
    let restore_result = set_android_stay_awake(toolchain, original);
    match (run_result, restore_result) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error).context("Android tests passed, but restoring the device stay-awake setting failed"),
        (Err(error), Err(restore_error)) => Err(error).with_context(|| {
            format!(
                "Android test failed and restoring the device stay-awake setting also failed: {restore_error:#}"
            )
        }),
    }
}

fn android_stay_awake_setting(toolchain: &AndroidToolchain) -> Result<u32> {
    let output = std::process::Command::new(&toolchain.adb)
        .args([
            "shell",
            "settings",
            "get",
            "global",
            "stay_on_while_plugged_in",
        ])
        .output()
        .context("Failed to read Android stay-awake setting with adb")?;
    if !output.status.success() {
        eyre::bail!(
            "adb could not read Android stay-awake setting: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    String::from_utf8(output.stdout)
        .context("Android stay-awake setting was not valid UTF-8")?
        .trim()
        .parse()
        .context("Android stay-awake setting was not an integer bitmask")
}

fn set_android_stay_awake(toolchain: &AndroidToolchain, setting: u32) -> Result<()> {
    let status = std::process::Command::new(&toolchain.adb)
        .args([
            "shell",
            "settings",
            "put",
            "global",
            "stay_on_while_plugged_in",
        ])
        .arg(setting.to_string())
        .status()
        .context("Failed to set Android stay-awake setting with adb")?;
    if !status.success() {
        eyre::bail!("adb could not set Android stay-awake setting to {setting}");
    }
    Ok(())
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

fn build_android_apk(root_dir: &Path, toolchain: &AndroidToolchain) -> Result<()> {
    info!("{}", "Building Android APK...".yellow().bold());
    let android_dir = root_dir.join("tests/android");
    let gradlew = android_dir.join("gradlew");
    let status = std::process::Command::new(&gradlew)
        .current_dir(&android_dir)
        .env("ANDROID_HOME", &toolchain.sdk_root)
        .env("ANDROID_SDK_ROOT", &toolchain.sdk_root)
        .arg(":app:assembleDebug")
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
        ],
        "location" | "permission" => &[
            "android.permission.ACCESS_FINE_LOCATION",
            "android.permission.ACCESS_COARSE_LOCATION",
        ],
        "camera" => &["android.permission.CAMERA"],
        "audio" | "speech" => &["android.permission.RECORD_AUDIO"],
        "contacts" => &["android.permission.READ_CONTACTS"],
        "calendar" => &["android.permission.READ_CALENDAR"],
        _ => &[],
    };

    for permission in permissions {
        run_adb(
            toolchain,
            ["shell", "pm", "grant", "com.waterkit.test", permission],
        )?;
    }

    Ok(())
}

fn launch_android_test(toolchain: &AndroidToolchain) -> Result<()> {
    run_adb(
        toolchain,
        ["shell", "am", "force-stop", "com.waterkit.test"],
    )?;
    run_adb(
        toolchain,
        [
            "shell",
            "run-as",
            "com.waterkit.test",
            "rm",
            "-f",
            "files/waterkit-test-report.json",
        ],
    )?;
    run_adb(
        toolchain,
        [
            "shell",
            "am",
            "start",
            "-n",
            "com.waterkit.test/.MainActivity",
            "--ez",
            "run_test",
            "true",
        ],
    )
}

fn wait_for_android_report(timeout: Duration, toolchain: &AndroidToolchain) -> Result<TestReport> {
    let deadline = Instant::now() + timeout;

    loop {
        let output = std::process::Command::new(&toolchain.adb)
            .args([
                "exec-out",
                "run-as",
                "com.waterkit.test",
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
            // nothing about why. Everything that would explain it — a crash on
            // start, a panic inside the harness activity, a sensor the emulator
            // does not provide — is in logcat, and the emulator is about to be
            // destroyed. Take it with us.
            let logcat = std::process::Command::new(&toolchain.adb)
                .args(["logcat", "-d", "-t", "300"])
                .output()
                .map_or_else(
                    |error| format!("<could not read logcat: {error}>"),
                    |logcat| String::from_utf8_lossy(&logcat.stdout).trim().to_string(),
                );
            eyre::bail!(
                "Timed out waiting for Android test report.\n\
                 last adb stderr: {}\n\
                 --- last 300 lines of logcat ---\n{logcat}",
                stderr.trim()
            );
        }

        thread::sleep(Duration::from_millis(250));
    }
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

fn ios_report_path(simulator_id: &str) -> Result<PathBuf> {
    let output = std::process::Command::new("xcrun")
        .args([
            "simctl",
            "get_app_container",
            simulator_id,
            "com.waterkit.test",
            "data",
        ])
        .output()
        .context("Failed to query iOS app data container")?;

    if !output.status.success() {
        eyre::bail!("Failed to query iOS app data container");
    }

    let container = String::from_utf8(output.stdout)
        .context("iOS app data container path was not valid UTF-8")?;
    Ok(PathBuf::from(container.trim()).join("Documents/waterkit-test-report.json"))
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
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{get_crate_feature, parse_android_min_sdk, sdk_root_from_adb};
    use std::path::{Path, PathBuf};

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
    fn parses_android_min_sdk_from_default_config() {
        let build_gradle = include_str!("../tests/fixtures/android-build.gradle.kts");
        assert_eq!(parse_android_min_sdk(build_gradle).as_deref(), Some("26"));
    }
}
