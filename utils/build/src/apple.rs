//! Apple platform build utilities.

#[cfg(any(target_os = "ios", target_os = "macos", test))]
use std::collections::BTreeSet;
use std::env;
#[cfg(any(target_os = "ios", target_os = "macos", test))]
use std::path::Path;
use std::path::PathBuf;

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
    /// Output library name (e.g., `CameraHelper`).
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

#[cfg(any(target_os = "ios", target_os = "macos", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppleTargetOs {
    Ios,
    Macos,
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
impl AppleTargetOs {
    fn from_cfg_target_os(target_os: &str) -> Option<Self> {
        match target_os {
            "ios" => Some(Self::Ios),
            "macos" => Some(Self::Macos),
            _ => None,
        }
    }
}

#[cfg(any(target_os = "ios", target_os = "macos", test))]
impl AppleTargetOs {
    fn matches_swift_os(self, name: &str) -> Option<bool> {
        match name {
            "iOS" => Some(matches!(self, Self::Ios)),
            "macOS" => Some(matches!(self, Self::Macos)),
            _ => None,
        }
    }
}

#[cfg(any(target_os = "ios", target_os = "macos", test))]
#[derive(Debug, Clone, Copy)]
struct SwiftConditionalFrame {
    parent_active: bool,
    branch_matched: bool,
    current_active: bool,
}

#[cfg(any(target_os = "ios", target_os = "macos", test))]
fn swift_bridge_static_lib_name(pkg_name: &str) -> String {
    format!("{}_swift_bridge", pkg_name.replace('-', "_"))
}

#[cfg(any(target_os = "ios", target_os = "macos", test))]
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

#[cfg(any(target_os = "ios", target_os = "macos", test))]
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

#[cfg(any(target_os = "ios", target_os = "macos", test))]
fn infer_swift_frameworks(swift_sources: &[PathBuf], target_os: AppleTargetOs) -> Vec<String> {
    let mut frameworks = BTreeSet::new();
    for source in swift_sources {
        let contents = std::fs::read_to_string(source)
            .unwrap_or_else(|_| panic!("Failed to read {}", source.display()));
        frameworks.extend(infer_swift_frameworks_from_source(&contents, target_os));
    }
    frameworks.into_iter().collect()
}

#[cfg(any(target_os = "ios", target_os = "macos", test))]
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
            let parent_active = frames.last().is_none_or(|frame| frame.current_active);
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

#[cfg(any(target_os = "ios", target_os = "macos", test))]
fn should_link_swift_import(module: &str) -> bool {
    !matches!(module, "Foundation" | "OSLog" | "ObjectiveC")
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
fn swift_runtime_lib_dir(swift_runtime_dir: &str) -> PathBuf {
    use std::process::Command;

    let swiftc_path = String::from_utf8(
        Command::new("xcrun")
            .args(["--find", "swiftc"])
            .output()
            .expect("xcrun --find swiftc failed")
            .stdout,
    )
    .expect("xcrun --find swiftc output must be UTF-8");

    PathBuf::from(swiftc_path.trim())
        .parent()
        .unwrap_or_else(|| panic!("swiftc path has no parent: {swiftc_path}"))
        .parent()
        .unwrap_or_else(|| panic!("swiftc bin path has no toolchain parent: {swiftc_path}"))
        .join(format!("lib/swift/{swift_runtime_dir}"))
}

/// The compiler-rt builtins archive name inside `lib/clang/*/lib/darwin` for a
/// given Swift runtime directory.
#[cfg(any(target_os = "ios", target_os = "macos"))]
fn clang_builtins_suffix(swift_runtime_dir: &str) -> Option<&'static str> {
    Some(match swift_runtime_dir {
        "macosx" => "osx",
        "iphoneos" => "ios",
        "iphonesimulator" => "iossim",
        "appletvos" => "tvos",
        "appletvsimulator" => "tvossim",
        "watchos" => "watchos",
        "watchsimulator" => "watchossim",
        "xros" => "xros",
        "xrsimulator" => "xrsim",
        _ => return None,
    })
}

/// `<toolchain>/usr/lib/clang/<ver>/lib/darwin` containing `libclang_rt.<suffix>.a`.
#[cfg(any(target_os = "ios", target_os = "macos"))]
fn clang_builtins_dir(swift_runtime_dir: &str, suffix: &str) -> Option<PathBuf> {
    let usr = swift_runtime_lib_dir(swift_runtime_dir)
        .ancestors()
        .nth(3)?
        .to_path_buf();
    let archive = format!("libclang_rt.{suffix}.a");
    for entry in std::fs::read_dir(usr.join("lib/clang")).ok()? {
        let darwin_dir = entry.ok()?.path().join("lib/darwin");
        if darwin_dir.join(&archive).is_file() {
            return Some(darwin_dir);
        }
    }
    None
}

/// Swift objects built with `#available` checks call compiler-rt builtins such
/// as `___isPlatformVersionAtLeast`. `swiftc` links `libclang_rt.<platform>.a`
/// implicitly when it drives a real app link; a `-nodefaultlibs` rustc-driven
/// link does not, so crates embedding Swift objects must declare it or the
/// dylib link fails with an undefined symbol (water-rs/waterui#929).
///
/// The library is declared as `static:-bundle`. A plain `static` link-lib
/// makes rustc merge the archive into `rlib`/`staticlib` artifacts, which
/// rejects clang's archive format ("Unsupported archive identifier"); the
/// `-bundle` modifier keeps the archive out of the rlib and instead records it
/// as a native dependency that Cargo carries to every downstream final link —
/// a `cdylib` app library built by the `water` CLI's static packaging path
/// included. `rustc-link-arg` cannot do that: Cargo applies a build script's
/// link args only to the emitting package's own targets, so it reached the
/// crate's dylib in the shared-library debug build and never the application's
/// release link, which failed with the undefined symbol
/// (water-rs/waterui#929 in one shape, the Apple nightly's `water package`
/// in the other). A `staticlib` output stays as before: nothing is bundled,
/// and the app link driven by Xcode's clang driver picks up the builtins on
/// its own.
#[cfg(any(target_os = "ios", target_os = "macos"))]
fn link_clang_builtins(swift_runtime_dir: &str) {
    let Some(suffix) = clang_builtins_suffix(swift_runtime_dir) else {
        println!(
            "cargo:warning=no clang builtins mapping for Swift runtime dir {swift_runtime_dir}"
        );
        return;
    };
    match clang_builtins_dir(swift_runtime_dir, suffix) {
        Some(dir) => {
            println!("cargo:rustc-link-search=native={}", dir.display());
            println!("cargo:rustc-link-lib=static:-bundle=clang_rt.{suffix}");
        }
        None => println!(
            "cargo:warning=libclang_rt.{suffix}.a not found under the active toolchain; Swift objects may fail to link"
        ),
    }
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
fn link_swift_runtime(swift_runtime_dir: &str) {
    let toolchain_lib = swift_runtime_lib_dir(swift_runtime_dir);
    println!("cargo:rustc-link-search=native={}", toolchain_lib.display());
    println!(
        "cargo:rustc-link-arg=-Wl,-rpath,{}",
        toolchain_lib.display()
    );

    if swift_runtime_dir == "macosx" {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }

    link_clang_builtins(swift_runtime_dir);
}

/// Reads an environment variable Cargo sets for every build script.
fn cargo_env(name: &str) -> String {
    env::var(name)
        .unwrap_or_else(|error| panic!("Cargo did not set {name} for the build script: {error}"))
}

/// Generate Swift bridge code from bridge modules.
///
/// This is for crates that only need bridge generation, not full Swift compilation.
///
/// # Arguments
/// * `bridges` - Iterator of paths to Rust bridge modules (e.g., "src/sys/apple/mod.rs")
///
/// # Panics
///
/// Panics when it does not run inside a Cargo build script, or when the Swift
/// sources next to a bridge cannot be read or compiled.
pub fn build_apple_bridge(bridges: impl IntoIterator<Item = impl AsRef<str>>) {
    let out_dir = PathBuf::from(cargo_env("OUT_DIR"));
    let manifest_dir = PathBuf::from(cargo_env("CARGO_MANIFEST_DIR"));
    let pkg_name = cargo_env("CARGO_PKG_NAME");

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

        let target_os = AppleTargetOs::from_cfg_target_os(&cargo_env("CARGO_CFG_TARGET_OS"))
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

/// The Swift toolchain coordinates of the Apple target Cargo is building for.
#[cfg(any(target_os = "ios", target_os = "macos"))]
struct SwiftTarget {
    /// The Rust target triple, as Cargo passes it in `TARGET`.
    rust_target: String,
    /// The SDK `xcrun --sdk` resolves.
    sdk: &'static str,
    /// The target triple `swiftc -target` takes.
    swift_triple: String,
    /// The directory of the Swift runtime under the toolchain's `lib/swift`.
    runtime_dir: &'static str,
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
impl SwiftTarget {
    fn from_cargo_env() -> Self {
        let rust_target = cargo_env("TARGET");
        let (sdk, swift_triple, runtime_dir) = if rust_target.contains("ios") {
            let arch = if rust_target.contains("x86_64") {
                "x86_64"
            } else {
                "arm64"
            };
            if rust_target.contains("ios-sim") {
                (
                    "iphonesimulator",
                    format!("{arch}-apple-ios14.0-simulator"),
                    "iphonesimulator",
                )
            } else {
                ("iphoneos", format!("{arch}-apple-ios14.0"), "iphoneos")
            }
        } else {
            let arch = if rust_target.contains("aarch64") || rust_target.contains("arm64") {
                "arm64"
            } else {
                "x86_64"
            };
            ("macosx", format!("{arch}-apple-macos12.3"), "macosx")
        };
        Self {
            rust_target,
            sdk,
            swift_triple,
            runtime_dir,
        }
    }

    fn sdk_path(&self) -> String {
        let output = std::process::Command::new("xcrun")
            .args(["--sdk", self.sdk, "--show-sdk-path"])
            .output()
            .expect("failed to run xcrun --show-sdk-path");
        assert!(
            output.status.success(),
            "xcrun --sdk {} --show-sdk-path failed: {}",
            self.sdk,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("xcrun --show-sdk-path output must be UTF-8")
            .trim()
            .to_string()
    }
}

/// Compiles the bridge code `swift-bridge` generated under `generated_name`,
/// together with `sources`, into the Swift module `module_name`, archives it
/// as `lib<module_name>.a`, and links that archive and the Swift runtime into
/// the crate.
#[cfg(any(target_os = "ios", target_os = "macos"))]
fn build_swift_library(
    out_dir: &Path,
    generated_name: &str,
    module_name: &str,
    sources: &[PathBuf],
) {
    use std::fs;
    use std::process::Command;

    let bridging_h = out_dir.join("Bridging-Header.h");
    let bridging_content = format!(
        "#include \"{}\"\n#include \"{}\"\n",
        out_dir.join("SwiftBridgeCore.h").display(),
        out_dir
            .join(format!("{generated_name}/{generated_name}.h"))
            .display()
    );
    fs::write(&bridging_h, bridging_content).expect("Failed to write bridging header");

    let generated = [
        out_dir.join("SwiftBridgeCore.swift"),
        out_dir.join(format!("{generated_name}/{generated_name}.swift")),
    ];
    let combined = generated
        .iter()
        .chain(sources)
        .map(|source| {
            fs::read_to_string(source)
                .unwrap_or_else(|error| panic!("Failed to read {}: {error}", source.display()))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let combined_swift = out_dir.join(format!("Combined{module_name}.swift"));
    fs::write(&combined_swift, combined).expect("Failed to write combined Swift file");

    let target = SwiftTarget::from_cargo_env();
    let obj_file = out_dir.join(format!("{module_name}.o"));
    let mut swiftc = Command::new("swiftc");
    swiftc
        .arg("-emit-object")
        .arg("-o")
        .arg(&obj_file)
        .arg("-sdk")
        .arg(target.sdk_path())
        .arg("-import-objc-header")
        .arg(&bridging_h)
        .arg("-parse-as-library")
        .arg("-module-name")
        .arg(module_name)
        .arg(&combined_swift)
        .arg("-target")
        .arg(&target.swift_triple);
    if has_ios26_background_task_apis(target.sdk, &target.rust_target) {
        swiftc.arg("-D").arg("WATERKIT_HAS_IOS26_BACKGROUND_TASKS");
    }

    let output = swiftc.output().expect("Failed to run swiftc");
    assert!(
        output.status.success(),
        "Swift compilation failed (swiftc args: {:?}):\n{}",
        swiftc.get_args().collect::<Vec<_>>(),
        String::from_utf8_lossy(&output.stderr)
    );

    let lib_file = out_dir.join(format!("lib{module_name}.a"));
    let ar_status = Command::new("ar")
        .arg("rcs")
        .arg(&lib_file)
        .arg(&obj_file)
        .status()
        .expect("Failed to run ar");
    assert!(ar_status.success(), "ar failed");

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static={module_name}");

    link_swift_runtime(target.runtime_dir);
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
///
/// # Panics
///
/// Panics when it does not run inside a Cargo build script, or when a Swift
/// source cannot be read or the Swift toolchain fails to compile or archive it.
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub fn compile_swift(bridge_rs: &str, config: &AppleSwiftConfig) {
    let out_dir = PathBuf::from(cargo_env("OUT_DIR"));
    let manifest_dir = PathBuf::from(cargo_env("CARGO_MANIFEST_DIR"));

    println!("cargo:rerun-if-changed={bridge_rs}");
    let sources: Vec<PathBuf> = config
        .swift_sources
        .iter()
        .map(|source| manifest_dir.join(source))
        .collect();
    for source in &sources {
        println!("cargo:rerun-if-changed={}", source.display());
    }

    swift_bridge_build::parse_bridges(vec![bridge_rs])
        .write_all_concatenated(&out_dir, &config.pkg_name);
    build_swift_library(&out_dir, &config.pkg_name, &config.lib_name, &sources);

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
/// * `lib_name` - Name for the output static library (e.g., `LocationTest`)
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
///
/// # Panics
///
/// Panics when it does not run inside a Cargo build script, or when a Swift
/// source cannot be read or the Swift toolchain fails to compile or archive it.
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub fn compile_multi_swift(lib_name: &str, crates: impl IntoIterator<Item = SwiftBridgeCrate>) {
    let out_dir = PathBuf::from(cargo_env("OUT_DIR"));
    let crates: Vec<SwiftBridgeCrate> = crates.into_iter().collect();

    for krate in &crates {
        println!("cargo:rerun-if-changed={}", krate.bridge_rs.display());
        for source in &krate.swift_sources {
            println!("cargo:rerun-if-changed={}", source.display());
        }
    }
    println!("cargo:rerun-if-changed=build.rs");

    let bridge_strings: Vec<String> = crates
        .iter()
        .map(|krate| krate.bridge_rs.to_string_lossy().into_owned())
        .collect();
    let bridges: Vec<&str> = bridge_strings.iter().map(String::as_str).collect();
    swift_bridge_build::parse_bridges(bridges).write_all_concatenated(&out_dir, lib_name);

    let sources: Vec<PathBuf> = crates
        .iter()
        .flat_map(|krate| krate.swift_sources.iter().cloned())
        .collect();
    build_swift_library(&out_dir, lib_name, lib_name, &sources);

    let frameworks: BTreeSet<&str> = std::iter::once("Foundation")
        .chain(
            crates
                .iter()
                .flat_map(|krate| krate.frameworks.iter().map(String::as_str)),
        )
        .collect();
    for framework in frameworks {
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
        let contents = r"
import Foundation
#if os(iOS)
import UIKit
import CoreHaptics
#elseif os(macOS)
import AppKit
#endif
import OSLog
";

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
