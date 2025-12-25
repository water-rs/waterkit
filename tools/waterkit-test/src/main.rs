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
    let content_toml_str = std::fs::read_to_string(&content_cargo_path).context("Read content toml")?;
    let content_doc = content_toml_str.parse::<DocumentMut>().context("Parse content toml")?;
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
    println!(
        "{}",
        "🚀 Preparing iOS test environment...".green().bold()
    );

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
    let content_toml_str = std::fs::read_to_string(&content_cargo_path).context("Read content toml")?;
    let content_doc = content_toml_str.parse::<DocumentMut>().context("Parse content toml")?;
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

    println!(
        "{}",
        "✅ iOS library built successfully.".green().bold()
    );
    println!("You can now run the app via Xcode in tests/ios/app");

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
