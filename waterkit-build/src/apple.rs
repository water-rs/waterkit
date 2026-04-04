//! Apple platform build utilities.

use std::collections::BTreeSet;
use std::env;
use std::path::{Path, PathBuf};

#[cfg(any(target_os = "ios", target_os = "macos"))]
fn has_ios26_background_task_apis(sdk: &str, target: &str) -> bool {
    if !target.contains("ios") {
        return false;
    }

    let output = std::process::Command::new("xcrun")
        .args(["--sdk", sdk, "--show-sdk-version"])
        .output();
    let Ok(output) = output else {
        return false;
    };
    if !output.status.success() {
        return false;
    }

    let Ok(version) = String::from_utf8(output.stdout) else {
        return false;
    };
    let major = version.trim().split('.').next();
    let Some(major) = major else {
        return false;
    };

    major.parse::<u32>().is_ok_and(|value| value >= 26)
}

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

/// A Swift bridge crate definition for multi-crate compilation.
///
/// Used with [`compile_multi_swift`] to compile Swift code from multiple
/// dependent crates into a single static library for a test binary.
#[derive(Debug, Clone)]
pub struct SwiftBridgeCrate {
    /// Path to the crate's bridge module (absolute path).
    pub bridge_rs: PathBuf,
    /// Swift source files to include (absolute paths).
    pub swift_sources: Vec<PathBuf>,
    /// Frameworks required by this crate.
    pub frameworks: Vec<String>,
}

impl SwiftBridgeCrate {
    /// Create a new Swift bridge crate definition.
    ///
    /// # Arguments
    /// * `bridge_rs` - Absolute path to the Rust bridge module (e.g., `crate/src/sys/apple/mod.rs`)
    #[must_use]
    pub fn new(bridge_rs: impl Into<PathBuf>) -> Self {
        Self {
            bridge_rs: bridge_rs.into(),
            swift_sources: Vec::new(),
            frameworks: Vec::new(),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppleTargetOs {
    Ios,
    Macos,
}

impl AppleTargetOs {
    fn from_cfg_target_os(target_os: &str) -> Option<Self> {
        match target_os {
            "ios" => Some(Self::Ios),
            "macos" => Some(Self::Macos),
            _ => None,
        }
    }

    fn matches_swift_os(self, name: &str) -> Option<bool> {
        match name {
            "iOS" => Some(matches!(self, Self::Ios)),
            "macOS" => Some(matches!(self, Self::Macos)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct SwiftConditionalFrame {
    parent_active: bool,
    branch_matched: bool,
    current_active: bool,
}

fn swift_bridge_static_lib_name(pkg_name: &str) -> String {
    format!("{}_swift_bridge", pkg_name.replace('-', "_"))
}

fn discover_swift_bridge_crates(
    manifest_dir: &Path,
    bridges: &[String],
    target_os: AppleTargetOs,
) -> Vec<SwiftBridgeCrate> {
    bridges
        .iter()
        .filter_map(|bridge| {
            let bridge_path = manifest_dir.join(bridge);
            let swift_sources = discover_swift_sources_for_bridge(&bridge_path);
            if swift_sources.is_empty() {
                return None;
            }

            let mut crate_def = SwiftBridgeCrate::new(bridge_path);
            let frameworks = infer_swift_frameworks(&swift_sources, target_os);
            for source in swift_sources {
                crate_def = crate_def.swift_source(source);
            }
            for framework in frameworks {
                crate_def = crate_def.framework(framework);
            }
            Some(crate_def)
        })
        .collect()
}

fn discover_swift_sources_for_bridge(bridge_path: &Path) -> Vec<PathBuf> {
    let Some(bridge_dir) = bridge_path.parent() else {
        return Vec::new();
    };

    let Ok(entries) = std::fs::read_dir(bridge_dir) else {
        return Vec::new();
    };

    let mut swift_sources = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "swift") {
                Some(path)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    swift_sources.sort();
    swift_sources
}

fn infer_swift_frameworks(swift_sources: &[PathBuf], target_os: AppleTargetOs) -> Vec<String> {
    let mut frameworks = BTreeSet::new();
    for source in swift_sources {
        let contents = std::fs::read_to_string(source)
            .unwrap_or_else(|_| panic!("Failed to read {}", source.display()));
        frameworks.extend(infer_swift_frameworks_from_source(&contents, target_os));
    }
    frameworks.into_iter().collect()
}

fn infer_swift_frameworks_from_source(
    contents: &str,
    target_os: AppleTargetOs,
) -> BTreeSet<String> {
    let mut frameworks = BTreeSet::new();
    let mut frames = Vec::<SwiftConditionalFrame>::new();

    for line in contents.lines() {
        let trimmed = line.trim();

        if let Some(condition) = trimmed.strip_prefix("#if os(") {
            let os_name = condition.strip_suffix(')').unwrap_or_else(|| {
                panic!("Unsupported Swift conditional directive: {trimmed}");
            });
            let parent_active = frames.last().map_or(true, |frame| frame.current_active);
            let current_active = parent_active
                && target_os.matches_swift_os(os_name).unwrap_or_else(|| {
                    panic!("Unsupported Swift target conditional os({os_name})");
                });
            frames.push(SwiftConditionalFrame {
                parent_active,
                branch_matched: current_active,
                current_active,
            });
            continue;
        }

        if let Some(condition) = trimmed.strip_prefix("#elseif os(") {
            let os_name = condition.strip_suffix(')').unwrap_or_else(|| {
                panic!("Unsupported Swift conditional directive: {trimmed}");
            });
            let frame = frames.last_mut().unwrap_or_else(|| {
                panic!("Encountered `#elseif` without matching `#if`: {trimmed}");
            });
            if !frame.parent_active || frame.branch_matched {
                frame.current_active = false;
            } else {
                let matches = target_os.matches_swift_os(os_name).unwrap_or_else(|| {
                    panic!("Unsupported Swift target conditional os({os_name})");
                });
                frame.current_active = matches;
                if matches {
                    frame.branch_matched = true;
                }
            }
            continue;
        }

        if trimmed == "#else" {
            let frame = frames.last_mut().unwrap_or_else(|| {
                panic!("Encountered `#else` without matching `#if`");
            });
            frame.current_active = frame.parent_active && !frame.branch_matched;
            frame.branch_matched = true;
            continue;
        }

        if trimmed == "#endif" {
            frames.pop().unwrap_or_else(|| {
                panic!("Encountered `#endif` without matching `#if`");
            });
            continue;
        }

        if frames.last().is_some_and(|frame| !frame.current_active) {
            continue;
        }

        let Some(module) = trimmed.strip_prefix("import ").map(str::trim) else {
            continue;
        };
        if should_link_swift_import(module) {
            frameworks.insert(module.to_string());
        }
    }

    assert!(
        frames.is_empty(),
        "Unclosed Swift conditional compilation block while inferring frameworks"
    );

    frameworks
}

fn should_link_swift_import(module: &str) -> bool {
    !matches!(module, "Foundation" | "OSLog" | "ObjectiveC")
}

/// Generate Swift bridge code from bridge modules.
///
/// This is for crates that only need bridge generation, not full Swift compilation.
///
/// # Arguments
/// * `bridges` - Iterator of paths to Rust bridge modules (e.g., "src/sys/apple/mod.rs")
pub fn build_apple_bridge(bridges: impl IntoIterator<Item = impl AsRef<str>>) {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let pkg_name = env::var("CARGO_PKG_NAME").unwrap();

    let bridges: Vec<String> = bridges
        .into_iter()
        .map(|b| b.as_ref().to_string())
        .collect();

    for bridge in &bridges {
        println!("cargo:rerun-if-changed={bridge}");
    }

    #[cfg(any(target_os = "ios", target_os = "macos"))]
    {
        let bridge_refs: Vec<&str> = bridges.iter().map(String::as_str).collect();
        swift_bridge_build::parse_bridges(bridge_refs).write_all_concatenated(out_dir, &pkg_name);

        let target_os = AppleTargetOs::from_cfg_target_os(
            &env::var("CARGO_CFG_TARGET_OS").expect("CARGO_CFG_TARGET_OS is required"),
        )
        .expect("build_apple_bridge only supports Apple targets");
        let swift_bridge_crates = discover_swift_bridge_crates(&manifest_dir, &bridges, target_os);
        if !swift_bridge_crates.is_empty() {
            compile_multi_swift(
                &swift_bridge_static_lib_name(&pkg_name),
                swift_bridge_crates,
            );
        }
    }

    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
    {
        let _ = (out_dir, manifest_dir, pkg_name, bridges);
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
    use std::fs;
    use std::process::Command;

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
    let (sdk, swift_target, swift_runtime_dir) = if target.contains("ios") {
        let is_simulator = target.contains("ios-sim") || target.contains("apple-ios-sim");
        let arch = if target.contains("x86_64") {
            "x86_64"
        } else {
            "arm64"
        };
        if is_simulator {
            (
                "iphonesimulator",
                format!("{arch}-apple-ios14.0-simulator"),
                "iphonesimulator",
            )
        } else {
            ("iphoneos", format!("{arch}-apple-ios14.0"), "iphoneos")
        }
    } else {
        // macOS
        let arch = if target.contains("aarch64") || target.contains("arm64") {
            "arm64"
        } else {
            "x86_64"
        };
        ("macosx", format!("{arch}-apple-macos12.3"), "macosx")
    };

    let sdk_path = String::from_utf8(
        Command::new("xcrun")
            .args(["--sdk", sdk, "--show-sdk-path"])
            .output()
            .expect("xcrun failed")
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let mut swiftc = Command::new("swiftc");
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
    swiftc.arg("-target").arg(&swift_target);
    if has_ios26_background_task_apis(sdk, &target) {
        swiftc.arg("-D").arg("WATERKIT_HAS_IOS26_BACKGROUND_TASKS");
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
        .join(format!("lib/swift/{swift_runtime_dir}"));
    println!("cargo:rustc-link-search=native={}", toolchain_lib.display());

    // Link required frameworks
    for framework in &config.frameworks {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}

/// No-op on non-Apple platforms.
#[cfg(not(any(target_os = "ios", target_os = "macos")))]
pub fn compile_swift(_bridge_rs: &str, _config: &AppleSwiftConfig) {}

/// Compile multiple Swift bridge crates into a single static library.
///
/// This is for test binaries that depend on multiple Swift-bridge crates.
/// It combines all bridge definitions and Swift sources into one compilation unit.
///
/// # Arguments
/// * `lib_name` - Name for the output static library (e.g., "LocationTest")
/// * `crates` - Iterator of Swift bridge crate definitions
///
/// # Example
///
/// ```ignore
/// use waterkit_build::{compile_multi_swift, SwiftBridgeCrate};
/// use std::path::PathBuf;
///
/// fn main() {
///     let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
///     let location = manifest_dir.join("../../../location");
///     let permission = manifest_dir.join("../../../permission");
///
///     compile_multi_swift("LocationTest", [
///         SwiftBridgeCrate::new(location.join("src/sys/apple/mod.rs"))
///             .swift_source(location.join("src/sys/apple/Location.swift"))
///             .framework("CoreLocation"),
///         SwiftBridgeCrate::new(permission.join("src/sys/apple/mod.rs"))
///             .swift_source(permission.join("src/sys/apple/Permission.swift"))
///             .framework("CoreLocation")
///             .framework("AVFoundation")
///             .framework("Photos")
///             .framework("Contacts")
///             .framework("EventKit"),
///     ]);
/// }
/// ```
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub fn compile_multi_swift(lib_name: &str, crates: impl IntoIterator<Item = SwiftBridgeCrate>) {
    use std::fs;
    use std::process::Command;

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let crates: Vec<SwiftBridgeCrate> = crates.into_iter().collect();

    // Collect all bridge paths and set up rerun-if-changed
    let bridge_strings: Vec<String> = crates
        .iter()
        .map(|c| c.bridge_rs.to_string_lossy().into_owned())
        .collect();

    for krate in &crates {
        println!("cargo:rerun-if-changed={}", krate.bridge_rs.display());
        for source in &krate.swift_sources {
            println!("cargo:rerun-if-changed={}", source.display());
        }
    }
    println!("cargo:rerun-if-changed=build.rs");

    let bridges: Vec<&str> = bridge_strings.iter().map(String::as_str).collect();

    // Generate Swift bridge code for all crates
    swift_bridge_build::parse_bridges(bridges).write_all_concatenated(&out_dir, lib_name);

    // Paths to generated files
    let core_swift = out_dir.join("SwiftBridgeCore.swift");
    let gen_swift = out_dir.join(format!("{lib_name}/{lib_name}.swift"));
    let core_h = out_dir.join("SwiftBridgeCore.h");
    let gen_h = out_dir.join(format!("{lib_name}/{lib_name}.h"));

    // Combine all Swift sources
    let mut combined =
        fs::read_to_string(&core_swift).expect("Failed to read SwiftBridgeCore.swift");
    combined.push('\n');
    combined.push_str(&fs::read_to_string(&gen_swift).expect("Failed to read generated swift"));

    for krate in &crates {
        for source in &krate.swift_sources {
            combined.push('\n');
            combined.push_str(
                &fs::read_to_string(source)
                    .unwrap_or_else(|_| panic!("Failed to read {}", source.display())),
            );
        }
    }

    let combined_swift = out_dir.join(format!("Combined{lib_name}.swift"));
    fs::write(&combined_swift, combined).expect("Failed to write combined Swift file");

    // Create bridging header
    let bridging_h = out_dir.join("Bridging-Header.h");
    let bridging_content = format!(
        "#include \"{}\"\n#include \"{}\"\n",
        core_h.display(),
        gen_h.display()
    );
    fs::write(&bridging_h, bridging_content).expect("Failed to write bridging header");

    // Get SDK path
    let target = env::var("TARGET").unwrap();
    let (sdk, swift_target, swift_runtime_dir) = if target.contains("ios") {
        let is_simulator = target.contains("ios-sim") || target.contains("apple-ios-sim");
        let arch = if target.contains("x86_64") {
            "x86_64"
        } else {
            "arm64"
        };
        if is_simulator {
            (
                "iphonesimulator",
                format!("{arch}-apple-ios14.0-simulator"),
                "iphonesimulator",
            )
        } else {
            ("iphoneos", format!("{arch}-apple-ios14.0"), "iphoneos")
        }
    } else {
        let arch = if target.contains("aarch64") || target.contains("arm64") {
            "arm64"
        } else {
            "x86_64"
        };
        ("macosx", format!("{arch}-apple-macos12.3"), "macosx")
    };

    let sdk_path = String::from_utf8(
        Command::new("xcrun")
            .args(["--sdk", sdk, "--show-sdk-path"])
            .output()
            .expect("xcrun failed")
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    // Compile Swift to object file
    let obj_file = out_dir.join(format!("{lib_name}.o"));

    let mut swiftc = Command::new("swiftc");
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
        .arg(lib_name)
        .arg(&combined_swift);

    // Add target triple
    swiftc.arg("-target").arg(&swift_target);
    if has_ios26_background_task_apis(sdk, &target) {
        swiftc.arg("-D").arg("WATERKIT_HAS_IOS26_BACKGROUND_TASKS");
    }

    let output = swiftc.output().expect("Failed to run swiftc");
    if !output.status.success() {
        eprintln!(
            "Swift compilation: swiftc args: {:?}",
            swiftc.get_args().collect::<Vec<_>>()
        );
        eprintln!("Swift compilation failed:");
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        panic!("Swift compilation failed");
    }

    // Create static library
    let lib_file = out_dir.join(format!("lib{lib_name}.a"));
    let ar_status = Command::new("ar")
        .args([
            "rcs",
            lib_file.to_str().unwrap(),
            obj_file.to_str().unwrap(),
        ])
        .status()
        .expect("Failed to run ar");
    assert!(ar_status.success(), "ar failed");

    // Link the static library
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static={lib_name}");

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
        .join(format!("lib/swift/{swift_runtime_dir}"));
    println!("cargo:rustc-link-search=native={}", toolchain_lib.display());

    // Collect and deduplicate frameworks, always include Foundation
    use std::collections::HashSet;
    let mut frameworks: HashSet<String> = HashSet::new();
    frameworks.insert("Foundation".to_string());
    for krate in &crates {
        for fw in &krate.frameworks {
            frameworks.insert(fw.clone());
        }
    }

    for framework in &frameworks {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}

/// No-op on non-Apple platforms.
#[cfg(not(any(target_os = "ios", target_os = "macos")))]
pub fn compile_multi_swift(_lib_name: &str, _crates: impl IntoIterator<Item = SwiftBridgeCrate>) {}

#[cfg(test)]
mod tests {
    use super::{
        AppleTargetOs, discover_swift_bridge_crates, infer_swift_frameworks_from_source,
        swift_bridge_static_lib_name,
    };

    #[test]
    fn infers_only_active_platform_frameworks_from_conditionals() {
        let contents = r#"
import Foundation
#if os(iOS)
import UIKit
import CoreHaptics
#elseif os(macOS)
import AppKit
#endif
import OSLog
"#;

        let ios = infer_swift_frameworks_from_source(contents, AppleTargetOs::Ios);
        assert!(ios.contains("UIKit"));
        assert!(ios.contains("CoreHaptics"));
        assert!(!ios.contains("AppKit"));
        assert!(!ios.contains("OSLog"));

        let macos = infer_swift_frameworks_from_source(contents, AppleTargetOs::Macos);
        assert!(macos.contains("AppKit"));
        assert!(!macos.contains("UIKit"));
        assert!(!macos.contains("CoreHaptics"));
    }

    #[test]
    fn discovers_swift_sources_next_to_bridge_module() {
        let root = std::env::temp_dir().join(format!(
            "waterkit-build-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time went backwards")
                .as_nanos()
        ));
        let apple_dir = root.join("src/sys/apple");
        std::fs::create_dir_all(&apple_dir).expect("create swift dir");
        std::fs::write(
            apple_dir.join("mod.rs"),
            "#[swift_bridge::bridge] mod ffi {}",
        )
        .expect("write bridge");
        std::fs::write(
            apple_dir.join("Feature.swift"),
            "import Foundation\n#if os(iOS)\nimport UIKit\n#endif\n",
        )
        .expect("write swift source");

        let discovered = discover_swift_bridge_crates(
            &root,
            &[String::from("src/sys/apple/mod.rs")],
            AppleTargetOs::Ios,
        );
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].swift_sources.len(), 1);
        assert!(discovered[0].frameworks.contains(&String::from("UIKit")));

        std::fs::remove_dir_all(&root).expect("cleanup temp dir");
    }

    #[test]
    fn sanitizes_generated_swift_bridge_library_name() {
        assert_eq!(
            swift_bridge_static_lib_name("waterkit-haptic"),
            "waterkit_haptic_swift_bridge"
        );
    }
}
