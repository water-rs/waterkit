use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use owo_colors::OwoColorize;
use std::path::{Path, PathBuf};
use toml_edit::DocumentMut;
use tracing::{info, warn};

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

    // 1. Verify crate path
    let crate_path = std::fs::canonicalize(crate_path).context("Failed to find crate path")?;

    if !crate_path.join("Cargo.toml").exists() {
        anyhow::bail!("No Cargo.toml found at {}", crate_path.display());
    }

    info!("Target crate: {}", crate_path.display());

    // 2. Resolve workspace root
    let root_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap() // tools
        .parent()
        .unwrap() // kit (root)
        .to_path_buf();

    // 3. Get feature
    let content_cargo_path = crate_path.join("Cargo.toml");
    let content_toml_str =
        std::fs::read_to_string(&content_cargo_path).context("Read content toml")?;
    let content_doc = content_toml_str
        .parse::<DocumentMut>()
        .context("Parse content toml")?;
    let package_name = content_doc["package"]["name"].as_str().unwrap_or("");
    let feature = get_crate_feature(package_name).ok_or_else(|| {
        anyhow::anyhow!("Unsupported crate package name for harness features: {package_name}")
    })?;

    // 4. Run cargo ndk build
    info!("{}", "Building Android test library...".yellow().bold());
    let mut args = vec![
        "ndk",
        "-t",
        "arm64-v8a",
        "-o",
        "tests/android/app/src/main/jniLibs",
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
        anyhow::bail!("Android build failed");
    }

    // 5. (Optional) Install/Run via adb/gradle could go here
    // For now we just build.
    info!("{}", "Android libraries built successfully.".green().bold());
    warn!("Install and launch are not automated in this command yet.");

    Ok(())
}

fn run_macos(crate_path: &Path) -> Result<()> {
    info!("{}", "Preparing macOS test environment...".green().bold());

    let crate_path = std::fs::canonicalize(crate_path).context("Failed to find crate path")?;
    let manifest_path = crate_path.join("Cargo.toml");
    if !manifest_path.exists() {
        anyhow::bail!("No Cargo.toml found at {}", crate_path.display());
    }

    let root_dir = workspace_root();
    let metadata = parse_macos_metadata(&manifest_path)?;
    info!("Target crate: {}", crate_path.display());
    info!("Package: {}", metadata.package_name);
    info!("Primary binary: {}", metadata.bin_name);

    info!("{}", "Building macOS test binary...".yellow().bold());
    let build_status = std::process::Command::new("cargo")
        .current_dir(&root_dir)
        .args(["build", "--manifest-path"])
        .arg(&manifest_path)
        .status()
        .context("Failed to run cargo build for macOS test crate")?;
    if !build_status.success() {
        anyhow::bail!("macOS build failed for {}", metadata.package_name);
    }

    let binary_path = root_dir.join("target/debug").join(&metadata.bin_name);
    if !binary_path.exists() {
        anyhow::bail!(
            "Built binary not found at {}. Ensure crate has a runnable binary target.",
            binary_path.display()
        );
    }

    let info_plist_path = crate_path.join("Info.plist");
    if info_plist_path.exists() {
        run_macos_app_bundle(
            &root_dir,
            &metadata.bin_name,
            &binary_path,
            &info_plist_path,
        )?;
    } else {
        run_macos_cli_binary(&root_dir, &manifest_path, &metadata.bin_name)?;
    }

    let log_path = root_dir
        .join("target/debug")
        .join(format!("{}.log", metadata.bin_name));
    if log_path.exists() {
        info!("{}", "Captured test log:".green().bold());
        let log_text = std::fs::read_to_string(&log_path)
            .with_context(|| format!("Failed to read {}", log_path.display()))?;
        info!("{log_text}");
    }

    Ok(())
}

fn run_ios(crate_path: &Path) -> Result<()> {
    info!("{}", "Preparing iOS test environment...".green().bold());

    // 1. Verify crate path
    let crate_path = std::fs::canonicalize(crate_path).context("Failed to find crate path")?;

    if !crate_path.join("Cargo.toml").exists() {
        anyhow::bail!("No Cargo.toml found at {}", crate_path.display());
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
        anyhow::anyhow!("Unsupported crate package name for harness features: {package_name}")
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
        anyhow::bail!("iOS build failed");
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
        anyhow::bail!("Swift compilation failed");
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
        anyhow::bail!("Codesign failed");
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
        anyhow::bail!("Installation failed (ensure a simulator is booted)");
    }

    info!("{}", "Launching app...".green().bold());
    let status = std::process::Command::new("xcrun")
        .args([
            "simctl",
            "launch",
            "--console",
            simulator_id,
            "com.waterkit.test",
        ])
        .current_dir(&root_dir)
        .status()
        .context("Failed to launch app")?;

    if !status.success() {
        anyhow::bail!("Launch failed");
    }

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
        .ok_or_else(|| anyhow::anyhow!("Missing package.name in {}", manifest_path.display()))?
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
        .ok_or_else(|| anyhow::anyhow!("First [[bin]] entry is missing"))?;
    let name = first["name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("First [[bin]] entry is missing a name field"))?;
    Ok(name.to_owned())
}

fn run_macos_cli_binary(root_dir: &Path, manifest_path: &Path, bin_name: &str) -> Result<()> {
    info!(
        "{}",
        "No Info.plist found; running binary directly via cargo run."
            .yellow()
            .bold()
    );

    let status = std::process::Command::new("cargo")
        .current_dir(root_dir)
        .args(["run", "--manifest-path"])
        .arg(manifest_path)
        .args(["--bin", bin_name])
        .status()
        .context("Failed to run cargo run for macOS test crate")?;

    if !status.success() {
        anyhow::bail!("macOS CLI run failed for binary {}", bin_name);
    }

    Ok(())
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
        anyhow::bail!("codesign failed for {}", app_dir.display());
    }

    info!("{}", "Launching app bundle...".green().bold());
    let open_status = std::process::Command::new("open")
        .arg("-W")
        .arg(&app_dir)
        .status()
        .context("Failed to run open -W for macOS app bundle")?;
    if !open_status.success() {
        anyhow::bail!("open -W failed for {}", app_dir.display());
    }

    Ok(())
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

    anyhow::bail!(
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
    if package_name.contains("sensor") {
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
