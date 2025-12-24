//! Apple platform build utilities.

use std::path::PathBuf;
use std::{env};

/// Configuration for Swift compilation.
#[derive(Debug, Clone)]
pub struct AppleSwiftConfig {
    /// The crate/module name (e.g., "waterkit-camera").
    pub pkg_name: String,
    /// Swift source files to compile.
    pub swift_sources: Vec<PathBuf>,
    /// Output library name (e.g., "CameraHelper").
    pub lib_name: String,
    /// Frameworks to link.
    pub frameworks: Vec<String>,
}

impl AppleSwiftConfig {
    /// Create a new config with required fields.
    #[must_use]
    pub fn new(pkg_name: impl Into<String>, lib_name: impl Into<String>) -> Self {
        Self {
            pkg_name: pkg_name.into(),
            swift_sources: Vec::new(),
            lib_name: lib_name.into(),
            frameworks: vec!["Foundation".to_string()],
        }
    }

    /// Add a Swift source file.
    #[must_use]
    pub fn swift_source(mut self, path: impl Into<PathBuf>) -> Self {
        self.swift_sources.push(path.into());
        self
    }

    /// Add a framework to link.
    #[must_use]
    pub fn framework(mut self, name: impl Into<String>) -> Self {
        self.frameworks.push(name.into());
        self
    }
}

/// Generate Swift bridge code from bridge modules.
///
/// This is for crates that only need bridge generation, not full Swift compilation.
///
/// # Arguments
/// * `bridges` - Slice of paths to Rust bridge modules (e.g., "src/sys/apple/mod.rs")
pub fn build_apple_bridge(bridges: &[&str]) {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let pkg_name = env::var("CARGO_PKG_NAME").unwrap();

    for bridge in bridges {
        println!("cargo:rerun-if-changed={bridge}");
    }

    #[cfg(any(target_os = "ios", target_os = "macos"))]
    {
        swift_bridge_build::parse_bridges(bridges.to_vec())
            .write_all_concatenated(out_dir, &pkg_name);
    }

    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
    {
        let _ = (out_dir, pkg_name);
    }
}

/// Compile Swift code and link it into the crate.
///
/// This handles:
/// 1. Swift bridge generation
/// 2. Creating bridging headers
/// 3. Compiling Swift to object file
/// 4. Creating static library
/// 5. Linking frameworks
///
/// # Arguments
/// * `bridge_rs` - Path to the Rust bridge module
/// * `config` - Swift compilation configuration
#[cfg(any(target_os = "ios", target_os = "macos"))]
#[allow(clippy::too_many_lines)]
pub fn compile_swift(bridge_rs: &str, config: &AppleSwiftConfig) {
    use std::process::Command;
    use std::fs;

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    // Track changes
    println!("cargo:rerun-if-changed={bridge_rs}");
    for source in &config.swift_sources {
        let full_path = manifest_dir.join(source);
        println!("cargo:rerun-if-changed={}", full_path.display());
    }

    // 1. Generate Swift bridge code
    swift_bridge_build::parse_bridges(vec![bridge_rs])
        .write_all_concatenated(out_dir.clone(), &config.pkg_name);

    // 2. Create combined bridging header
    let core_h = out_dir.join("SwiftBridgeCore.h");
    let pkg_h = out_dir.join(format!("{}/{}.h", config.pkg_name, config.pkg_name));
    let bridging_h = out_dir.join("Bridging-Header.h");

    let bridging_content = format!(
        "#include \"{}\"\n#include \"{}\"\n",
        core_h.display(),
        pkg_h.display()
    );
    fs::write(&bridging_h, bridging_content).expect("Failed to write bridging header");

    // 3. Concatenate all Swift sources into one file
    let core_swift = out_dir.join("SwiftBridgeCore.swift");
    let gen_swift = out_dir.join(format!("{}/{}.swift", config.pkg_name, config.pkg_name));
    let combined_swift = out_dir.join(format!("Combined{}.swift", config.lib_name));

    let mut combined_content =
        fs::read_to_string(&core_swift).expect("Failed to read SwiftBridgeCore.swift");
    combined_content.push('\n');
    combined_content
        .push_str(&fs::read_to_string(&gen_swift).expect("Failed to read generated swift"));

    for source in &config.swift_sources {
        let full_path = manifest_dir.join(source);
        combined_content.push('\n');
        combined_content.push_str(
            &fs::read_to_string(&full_path)
                .unwrap_or_else(|_| panic!("Failed to read {}", full_path.display())),
        );
    }

    fs::write(&combined_swift, combined_content).expect("Failed to write combined Swift file");

    // 4. Compile Swift to object file
    let obj_file = out_dir.join(format!("{}.o", config.lib_name));

    let target = env::var("TARGET").unwrap();
    let sdk = if target.contains("ios") {
        "iphoneos"
    } else {
        "macosx"
    };

    let sdk_path = String::from_utf8(
        ::new("xcrun")
            .args(["--sdk", sdk, "--show-sdk-path"])
            .output()
            .expect("xcrun failed")
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let mut swiftc = ::new("swiftc");
    swiftc
        .arg("-emit-object")
        .arg("-o")
        .arg(&obj_file)
        .arg("-sdk")
        .arg(&sdk_path)
        .arg("-import-objc-header")
        .arg(&bridging_h)
        .arg("-parse-as-library")
        .arg("-module-name")
        .arg(&config.lib_name)
        .arg(&combined_swift);

    // Add target triple for cross-compilation
    if target.contains("ios") {
        swiftc.arg("-target").arg("arm64-apple-ios14.0");
    } else if target.contains("aarch64") {
        swiftc.arg("-target").arg("arm64-apple-macos12.3");
    } else {
        swiftc.arg("-target").arg("x86_64-apple-macos12.3");
    }

    let output = swiftc.output().expect("Failed to run swiftc");
    if !output.status.success() {
        eprintln!(
            "Swift compilation : swiftc args: {:?}",
            swiftc.get_args().collect::<Vec<_>>()
        );
        eprintln!("Swift compilation failed:");
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        panic!("Swift compilation failed");
    }

    // Create static library from object file
    let lib_file = out_dir.join(format!("lib{}.a", config.lib_name));
    let ar_status = Command::new("ar")
        .args([
            "rcs",
            lib_file.to_str().unwrap(),
            obj_file.to_str().unwrap(),
        ])
        .status()
        .expect("Failed to run ar");
    assert!(ar_status.success(), "ar failed");

    // Link the Swift library
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static={}", config.lib_name);

    // Link Swift runtime
    let toolchain_dir = String::from_utf8(
        Command::new("xcrun")
            .args(["--find", "swiftc"])
            .output()
            .expect("xcrun --find swiftc failed")
            .stdout,
    )
    .unwrap();
    let toolchain_lib = PathBuf::from(toolchain_dir.trim())
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("lib/swift/macosx");
    println!("cargo:rustc-link-search=native={}", toolchain_lib.display());

    // Link required frameworks
    for framework in &config.frameworks {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}

/// No-op on non-Apple platforms.
#[cfg(not(any(target_os = "ios", target_os = "macos")))]
pub fn compile_swift(_bridge_rs: &str, _config: &AppleSwiftConfig) {}
