use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use owo_colors::OwoColorize;
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item, Value};

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
    let cli = Cli::parse();

    match cli.command {
        Commands::Android { crate_path } => run_android(&crate_path),
        Commands::Macos { crate_path } => run_macos(&crate_path),
        Commands::Ios { crate_path } => run_ios(&crate_path),
    }
}

fn run_android(crate_path: &Path) -> Result<()> {
    println!(
        "{}",
        "🚀 Preparing Android test environment...".green().bold()
    );

    // 1. Verify crate path
    let crate_path = std::fs::canonicalize(crate_path).context("Failed to find crate path")?;

    if !crate_path.join("Cargo.toml").exists() {
        anyhow::bail!("No Cargo.toml found at {}", crate_path.display());
    }

    println!("Target crate: {}", crate_path.display());

    // 2. Modify tests/android/rust/Cargo.toml
    let root_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap() // tools
        .parent()
        .unwrap() // kit (root)
        .to_path_buf();

    let harness_cargo_path = root_dir.join("tests/android/rust/Cargo.toml");
    update_harness_dependency(&harness_cargo_path, &crate_path)?;

    // 2.5 Get feature
    let content_cargo_path = crate_path.join("Cargo.toml");
    let content_toml_str =
        std::fs::read_to_string(&content_cargo_path).context("Read content toml")?;
    let content_doc = content_toml_str
        .parse::<DocumentMut>()
        .context("Parse content toml")?;
    let package_name = content_doc["package"]["name"].as_str().unwrap_or("");
    let feature = get_crate_feature(package_name);

    // 3. Run cargo ndk build
    println!("{}", "🔨 Building Android app...".yellow().bold());
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
    if let Some(f) = feature {
        args.push("--features");
        args.push(f);
    }

    let status = std::process::Command::new("cargo")
        .current_dir(&root_dir)
        .args(&args)
        .status()
        .context("Failed to run cargo ndk")?;

    if !status.success() {
        anyhow::bail!("Android build failed");
    }

    // 4. (Optional) Install/Run via adb/gradle could go here
    // For now we just build.
    println!(
        "{}",
        "✅ Android libraries built successfully.".green().bold()
    );
    println!("You can now run the app via Android Studio or ./gradlew installDebug");

    Ok(())
}

fn run_macos(crate_path: &Path) -> Result<()> {
    println!(
        "{}",
        "🚀 Preparing macOS test environment...".green().bold()
    );

    // 1. Verify crate path
    let crate_path = std::fs::canonicalize(crate_path).context("Failed to find crate path")?;

    if !crate_path.join("Cargo.toml").exists() {
        anyhow::bail!("No Cargo.toml found at {}", crate_path.display());
    }

    // 2. Modify tests/macos/runner/Cargo.toml
    // Implementation needed: Create generic macOS runner crate.
    // For now, let's just log.
    println!(
        "{}",
        "⚠️ macOS generic runner not fully implemented yet.".yellow()
    );
    println!("Target crate: {}", crate_path.display());

    Ok(())
}

fn run_ios(crate_path: &Path) -> Result<()> {
    println!("{}", "🚀 Preparing iOS test environment...".green().bold());

    // 1. Verify crate path
    let crate_path = std::fs::canonicalize(crate_path).context("Failed to find crate path")?;

    if !crate_path.join("Cargo.toml").exists() {
        anyhow::bail!("No Cargo.toml found at {}", crate_path.display());
    }

    println!("Target crate: {}", crate_path.display());

    // 2. Modify tests/ios/rust/Cargo.toml
    let root_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap() // tools
        .parent()
        .unwrap() // kit (root)
        .to_path_buf();

    let harness_cargo_path = root_dir.join("tests/ios/rust/Cargo.toml");
    update_harness_dependency(&harness_cargo_path, &crate_path)?;

    // 2.5 Get feature
    let content_cargo_path = crate_path.join("Cargo.toml");
    let content_toml_str =
        std::fs::read_to_string(&content_cargo_path).context("Read content toml")?;
    let content_doc = content_toml_str
        .parse::<DocumentMut>()
        .context("Parse content toml")?;
    let package_name = content_doc["package"]["name"].as_str().unwrap_or("");
    let feature = get_crate_feature(package_name);

    // 3. Build for iOS Simulator
    println!("{}", "🔨 Building iOS library...".yellow().bold());
    let mut args = vec![
        "build",
        "--target",
        "aarch64-apple-ios-sim",
        "-p",
        "waterkit-test-ios",
    ];
    if let Some(f) = feature {
        args.push("--features");
        args.push(f);
    }

    let status = std::process::Command::new("cargo")
        .current_dir(&root_dir)
        .args(&args)
        .status()
        .context("Failed to run cargo build")?;

    if !status.success() {
        anyhow::bail!("iOS build failed");
    }

    // 4. Swift Compile
    println!("{}", "🍎 Compiling Swift app...".yellow().bold());

    // Scan for extra .swift sources in the target crate
    let mut extra_swift_sources = Vec::new();
    let sys_apple_dir = crate_path.join("src/sys/apple");
    #[allow(clippy::collapsible_if)]
    if sys_apple_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(sys_apple_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "swift") {
                    println!("Found extra Swift source: {}", path.display());
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
    println!("{}", "📦 Bundling app...".yellow().bold());
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
    println!("{}", "🔑 Codesigning...".yellow().bold());
    let status = std::process::Command::new("codesign")
        .args(["-s", "-", "WaterKitTest.app"])
        .current_dir(&root_dir)
        .status()
        .context("Failed to codesign")?;

    if !status.success() {
        anyhow::bail!("Codesign failed");
    }

    // 7. Install & Launch
    println!(
        "{}",
        "📱 Installing to Simulator (Booted)...".yellow().bold()
    );
    let simulator_id = "booted"; // Use "booted" to target the active simulator automatically!

    let status = std::process::Command::new("xcrun")
        .args(["simctl", "install", simulator_id, "WaterKitTest.app"])
        .current_dir(&root_dir)
        .status()
        .context("Failed to install to simulator")?;

    if !status.success() {
        anyhow::bail!("Installation failed (ensure a simulator is booted)");
    }

    println!("{}", "🚀 Launching app...".green().bold());
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

fn update_harness_dependency(harness_path: &Path, content_crate_path: &Path) -> Result<()> {
    // 1. Get crate name from content crate Cargo.toml
    let content_cargo_path = content_crate_path.join("Cargo.toml");
    let content_toml_str = std::fs::read_to_string(&content_cargo_path)
        .context("Failed to read content Cargo.toml")?;
    let content_doc = content_toml_str
        .parse::<DocumentMut>()
        .context("Failed to parse content Cargo.toml")?;

    let package_name = content_doc["package"]["name"].as_str().ok_or_else(|| {
        anyhow::anyhow!(
            "Failed to get package name from {}",
            content_cargo_path.display()
        )
    })?;

    // 2. Update harness Cargo.toml
    let toml_str =
        std::fs::read_to_string(harness_path).context("Failed to read harness Cargo.toml")?;

    let mut doc = toml_str
        .parse::<DocumentMut>()
        .context("Failed to parse harness Cargo.toml")?;

    // We want to add/update: waterkit_content = { package = "name", path = "..." }

    let path_str = content_crate_path.to_str().unwrap();

    let mut table = toml_edit::InlineTable::default();
    table.insert("path", Value::from(path_str));
    table.insert("package", Value::from(package_name));

    doc["dependencies"]["waterkit_content"] = Item::Value(Value::InlineTable(table));

    println!("DEBUG: Generated TOML content for [dependencies.waterkit_content]:");
    println!("{}", doc["dependencies"]["waterkit_content"]);

    std::fs::write(harness_path, doc.to_string()).context("Failed to write harness Cargo.toml")?;

    println!(
        "Updated harness dependency to: {} (package: {})",
        path_str, package_name
    );
    Ok(())
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
    } else {
        None
    }
}
