use clap::{Parser, Subcommand};
use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Formatted, Item, Value};

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
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Android { crate_path } => run_android(&crate_path),
        Commands::Macos { crate_path } => run_macos(&crate_path),
    }
}

fn run_android(crate_path: &Path) -> Result<()> {
    println!("{}", "🚀 Preparing Android test environment...".green().bold());
    
    // 1. Verify crate path
    let crate_path = std::fs::canonicalize(crate_path)
        .context("Failed to find crate path")?;
    
    if !crate_path.join("Cargo.toml").exists() {
        anyhow::bail!("No Cargo.toml found at {}", crate_path.display());
    }

    println!("Target crate: {}", crate_path.display());

    // 2. Modify tests/android/rust/Cargo.toml
    let root_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap() // tools
        .parent().unwrap() // kit (root)
        .to_path_buf();
        
    let harness_cargo_path = root_dir.join("tests/android/rust/Cargo.toml");
    update_harness_dependency(&harness_cargo_path, &crate_path)?;

    // 3. Run cargo ndk build
    println!("{}", "🔨 Building Android app...".yellow().bold());
    let status = std::process::Command::new("cargo")
        .current_dir(root_dir) // Run from root
        .args(&[
            "ndk", 
            "-t", "arm64-v8a", 
            "-o", "tests/android/app/src/main/jniLibs", 
            "build", 
            "-p", "waterkit-test-android" // The harness crate
        ])
        .status()
        .context("Failed to run cargo ndk")?;

    if !status.success() {
        anyhow::bail!("Android build failed");
    }

    // 4. (Optional) Install/Run via adb/gradle could go here
    // For now we just build.
    println!("{}", "✅ Android libraries built successfully.".green().bold());
    println!("You can now run the app via Android Studio or ./gradlew installDebug");

    Ok(())
}

fn run_macos(crate_path: &Path) -> Result<()> {
    println!("{}", "🚀 Preparing macOS test environment...".green().bold());
    
     // 1. Verify crate path
    let crate_path = std::fs::canonicalize(crate_path)
        .context("Failed to find crate path")?;
    
    if !crate_path.join("Cargo.toml").exists() {
        anyhow::bail!("No Cargo.toml found at {}", crate_path.display());
    }

    // 2. Modify tests/macos/runner/Cargo.toml
    // Implementation needed: Create generic macOS runner crate.
    // For now, let's just log.
    println!("{}", "⚠️ macOS generic runner not fully implemented yet.".yellow());
    println!("Target crate: {}", crate_path.display());
    
    Ok(())
}

fn update_harness_dependency(harness_path: &Path, content_crate_path: &Path) -> Result<()> {
    let toml_str = std::fs::read_to_string(harness_path)
        .context("Failed to read harness Cargo.toml")?;
        
    let mut doc = toml_str.parse::<DocumentMut>()
        .context("Failed to parse harness Cargo.toml")?;

    // We assume the harness has [dependencies] section
    // We want to add/update: waterkit_content = { path = "..." }
    
    let path_str = content_crate_path.to_str().unwrap();
    
    // Using inline table for { path = "..." }
    let mut table = toml_edit::InlineTable::default();
    table.insert("path", Value::from(path_str));
    
    doc["dependencies"]["waterkit_content"] = Item::Value(Value::InlineTable(table));
    
    std::fs::write(harness_path, doc.to_string())
        .context("Failed to write harness Cargo.toml")?;
        
    println!("Updated harness dependency to: {}", path_str);
    Ok(())
}
