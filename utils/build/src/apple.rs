//! Apple platform build utilities.

#[cfg(any(target_os = "ios", target_os = "macos"))]
use std::collections::BTreeSet;
#[cfg(any(target_os = "ios", target_os = "macos"))]
use std::env;
use std::path::{Path, PathBuf};

#[cfg(any(target_os = "ios", target_os = "macos"))]
fn has_ios26_background_task_apis(target: &SwiftTarget) -> bool {
    // The iOS 26 continued-processing task APIs are unavailable in Mac
    // Catalyst, whatever the macOS SDK version.
    if !target.rust_target.contains("ios") || target.mac_catalyst {
        return false;
    }

    let output = std::process::Command::new("xcrun")
        .args(["--sdk", target.sdk, "--show-sdk-version"])
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

/// One `#[swift_bridge::bridge]` module of a crate and the Swift sources and
/// frameworks that implement its `extern "Swift"` side.
///
/// The module and every source are crate-relative paths, resolved against
/// `CARGO_MANIFEST_DIR` by [`SwiftBridges`].
#[derive(Debug, Clone)]
pub struct SwiftBridge {
    module: PathBuf,
    sources: Vec<PathBuf>,
    frameworks: Vec<String>,
}

impl SwiftBridge {
    /// The crate-relative path of the `#[swift_bridge::bridge]` module, such
    /// as `src/sys/apple/mod.rs`.
    #[must_use]
    pub fn new(module: impl Into<PathBuf>) -> Self {
        Self {
            module: module.into(),
            sources: Vec::new(),
            frameworks: Vec::new(),
        }
    }

    /// Adds a Swift source implementing the bridge's `extern "Swift"` side, as
    /// a crate-relative path.
    #[must_use]
    pub fn swift_source(mut self, path: impl Into<PathBuf>) -> Self {
        self.sources.push(path.into());
        self
    }

    /// Adds an Apple framework the bridge's Swift code links against. A build
    /// script picks frameworks per target from `CARGO_CFG_TARGET_OS` and
    /// `CARGO_CFG_TARGET_ABI`.
    #[must_use]
    pub fn framework(mut self, name: impl Into<String>) -> Self {
        self.frameworks.push(name.into());
        self
    }
}

/// All of a crate's Swift bridges.
#[derive(Debug, Clone, Default)]
pub struct SwiftBridges {
    bridges: Vec<SwiftBridge>,
}

impl SwiftBridges {
    /// An empty builder. Add each of the crate's bridges with
    /// [`Self::bridge`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a bridge to the builder.
    #[must_use]
    pub fn bridge(mut self, bridge: SwiftBridge) -> Self {
        self.bridges.push(bridge);
        self
    }
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
impl SwiftBridges {
    /// Generates swift-bridge's core and every bridge's glue once, compiles
    /// them with the bridges' Swift sources into one static library named
    /// after `CARGO_PKG_NAME`, links each declared framework, and emits
    /// `rerun-if-changed` for every module and Swift source.
    ///
    /// # Panics
    ///
    /// Panics when it does not run inside a Cargo build script, when the
    /// builder holds no bridges, when a bridge declares no Swift sources, or
    /// when a Swift source cannot be read or the Swift toolchain fails to
    /// compile or archive it.
    pub fn compile(self) {
        let manifest_dir = PathBuf::from(cargo_env("CARGO_MANIFEST_DIR"));
        let pkg_name = cargo_env("CARGO_PKG_NAME");
        let out_dir = PathBuf::from(cargo_env("OUT_DIR"));

        assert!(
            !self.bridges.is_empty(),
            "SwiftBridges::compile() requires at least one bridge; \
             add each of the crate's bridge modules with .bridge(SwiftBridge::new(..))"
        );
        for bridge in &self.bridges {
            assert!(
                !bridge.sources.is_empty(),
                "Swift bridge {} declares no Swift sources; \
                 point .swift_source() at the files implementing its extern \"Swift\" side",
                bridge.module.display(),
            );
        }

        self.emit_rerun_if_changed(&manifest_dir);

        let modules = self.module_paths(&manifest_dir);
        swift_bridge_build::parse_bridges(&modules).write_all_concatenated(&out_dir, &pkg_name);

        let sources: Vec<PathBuf> = self
            .bridges
            .iter()
            .flat_map(|bridge| bridge.sources.iter().map(|s| manifest_dir.join(s)))
            .collect();
        let lib_name = pkg_name.replace('-', "_");
        build_swift_library(&out_dir, &pkg_name, &lib_name, &sources);

        let frameworks: BTreeSet<&str> = self
            .bridges
            .iter()
            .flat_map(|bridge| bridge.frameworks.iter().map(String::as_str))
            .collect();
        for framework in frameworks {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    }

    /// Writes the generated Swift glue and headers into `dir` without
    /// compiling, for an app target that compiles them itself: swift-bridge's
    /// `SwiftBridgeCore`, the crate's bridge glue, and a `Bridging-Header.h`
    /// including both. A relative `dir` resolves against
    /// `CARGO_MANIFEST_DIR`.
    ///
    /// # Panics
    ///
    /// Panics when it does not run inside a Cargo build script, when the
    /// builder holds no bridges, or when `dir` cannot be created or written.
    pub fn generate_into(self, dir: impl AsRef<Path>) {
        let manifest_dir = PathBuf::from(cargo_env("CARGO_MANIFEST_DIR"));
        let pkg_name = cargo_env("CARGO_PKG_NAME");

        assert!(
            !self.bridges.is_empty(),
            "SwiftBridges::generate_into() requires at least one bridge; \
             add each of the crate's bridge modules with .bridge(SwiftBridge::new(..))"
        );

        let dir = manifest_dir.join(dir.as_ref());
        std::fs::create_dir_all(&dir).unwrap_or_else(|error| {
            panic!(
                "failed to create Swift bridge output dir {}: {error}",
                dir.display()
            )
        });

        self.emit_rerun_if_changed(&manifest_dir);

        let modules = self.module_paths(&manifest_dir);
        swift_bridge_build::parse_bridges(&modules).write_all_concatenated(&dir, &pkg_name);

        let bridging_header =
            format!("#include \"SwiftBridgeCore.h\"\n#include \"{pkg_name}/{pkg_name}.h\"\n");
        std::fs::write(dir.join("Bridging-Header.h"), bridging_header).unwrap_or_else(|error| {
            panic!(
                "failed to write {}: {error}",
                dir.join("Bridging-Header.h").display()
            )
        });
    }

    /// The bridge module paths, resolved against `manifest_dir`.
    fn module_paths(&self, manifest_dir: &Path) -> Vec<PathBuf> {
        self.bridges
            .iter()
            .map(|bridge| manifest_dir.join(&bridge.module))
            .collect()
    }

    /// Tracks the build script, every bridge module and every Swift source for
    /// rebuilds.
    fn emit_rerun_if_changed(&self, manifest_dir: &Path) {
        println!("cargo:rerun-if-changed=build.rs");
        for bridge in &self.bridges {
            println!(
                "cargo:rerun-if-changed={}",
                manifest_dir.join(&bridge.module).display()
            );
            for source in &bridge.sources {
                println!(
                    "cargo:rerun-if-changed={}",
                    manifest_dir.join(source).display()
                );
            }
        }
    }
}

/// Swift is generated and compiled with the Xcode toolchain, which only a
/// macOS host has.
#[cfg(not(any(target_os = "ios", target_os = "macos")))]
impl SwiftBridges {
    /// Compiling Swift requires the Xcode toolchain.
    ///
    /// # Panics
    ///
    /// Always: a build script reaches this only when it targets an Apple
    /// platform from a host that cannot build for one.
    pub fn compile(self) {
        panic!(
            "compiling the Swift bridges [{}] requires a macOS host with Xcode",
            self.summary()
        );
    }

    /// Generating bridge glue parses the modules with `swift-bridge-build`,
    /// which `waterkit-build` links only on an Apple host.
    ///
    /// # Panics
    ///
    /// Always: a build script reaches this only when it targets an Apple
    /// platform from a host that cannot generate for one.
    pub fn generate_into(self, dir: impl AsRef<Path>) {
        panic!(
            "generating the Swift bridges [{}] into {} requires a macOS host with Xcode",
            self.summary(),
            dir.as_ref().display(),
        );
    }

    /// The declared bridges, for the unsupported-host panic messages.
    fn summary(&self) -> String {
        self.bridges
            .iter()
            .map(|bridge| {
                format!(
                    "{} ({} sources, {} frameworks)",
                    bridge.module.display(),
                    bridge.sources.len(),
                    bridge.frameworks.len()
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// An OS version as `major.minor[.patch]`, ordered the way deployment targets
/// compare.
#[cfg(any(target_os = "ios", target_os = "macos", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct OsVersion {
    major: u32,
    minor: u32,
    patch: u32,
}

#[cfg(any(target_os = "ios", target_os = "macos", test))]
impl OsVersion {
    const fn new(major: u32, minor: u32) -> Self {
        Self {
            major,
            minor,
            patch: 0,
        }
    }

    fn parse(text: &str) -> Option<Self> {
        let mut components = text.split('.');
        let component = |part: Option<&str>| part.map_or(Some(0), |part| part.parse().ok());
        Some(Self {
            major: component(components.next())?,
            minor: component(components.next())?,
            patch: component(components.next())?,
        })
    }
}

#[cfg(any(target_os = "ios", target_os = "macos", test))]
impl std::fmt::Display for OsVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.patch == 0 {
            write!(f, "{}.{}", self.major, self.minor)
        } else {
            write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
        }
    }
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

/// The deployment target rustc stamps on binaries built for `rust_target`, as
/// the `VAR=value` line `rustc --print deployment-target` reports.
///
/// Asking rustc — rather than reading the environment — gives the effective
/// value: a `*_DEPLOYMENT_TARGET` variable overrides the target's default, and
/// the print reflects both.
#[cfg(any(target_os = "ios", target_os = "macos"))]
fn rustc_deployment_target(rust_target: &str) -> (String, OsVersion) {
    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let output = std::process::Command::new(rustc)
        .args(["--print", "deployment-target", "--target", rust_target])
        .output()
        .expect("failed to run rustc --print deployment-target");
    assert!(
        output.status.success(),
        "rustc --print deployment-target --target {rust_target} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("rustc --print output must be UTF-8");
    let line = stdout.trim();
    let (var, version) = line.split_once('=').unwrap_or_else(|| {
        panic!("rustc --print deployment-target printed {line:?}, not NAME=value")
    });
    let version = OsVersion::parse(version)
        .unwrap_or_else(|| panic!("rustc reported an unparsable deployment target {version:?}"));
    (var.to_string(), version)
}

/// Fails the build when the deployment target rustc will stamp on binaries is
/// below `target.deployment`, the floor the Swift objects are compiled for.
///
/// `ld` consults the SDK stubs' back-deployment rules against the *binary's*
/// deployment version, not the objects': below the version an OS shipped a
/// Swift runtime library, the stub records the `@rpath` install name a bundle
/// embedding the library would satisfy. `rustc` defaults
/// `aarch64-apple-darwin` to macOS 11, below the Swift concurrency runtime's
/// in-OS floor of macOS 12, so a binary linking those objects — the `waterkit`
/// facade's test binary is one — referenced `@rpath/libswift_Concurrency.dylib`
/// and aborted in dyld with no `LC_RPATH`. A library build script cannot raise
/// a dependent's deployment target (`rustc-link-arg` does not propagate), so
/// the mismatch must fail here, naming the `*_DEPLOYMENT_TARGET` override the
/// consumer controls.
#[cfg(any(target_os = "ios", target_os = "macos"))]
fn assert_deployment_floor(target: &SwiftTarget) {
    let (env_var, effective) = rustc_deployment_target(&target.rust_target);
    println!("cargo:rerun-if-env-changed={env_var}");
    let pkg = cargo_env("CARGO_PKG_NAME");
    assert!(
        effective >= target.deployment,
        "{pkg} compiles Swift code with deployment target {}, but rustc links \
         {} binaries with {env_var}={effective}, below that floor: the linker \
         records @rpath install names for Swift runtime libraries, and every \
         binary linking the crate aborts in dyld at launch. Set {env_var}={} \
         or newer.",
        target.deployment,
        target.rust_target,
        target.deployment,
    );
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
#[cfg(any(target_os = "ios", target_os = "macos"))]
fn cargo_env(name: &str) -> String {
    env::var(name)
        .unwrap_or_else(|error| panic!("Cargo did not set {name} for the build script: {error}"))
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
    /// The deployment version `swift_triple` encodes: the oldest OS version the
    /// compiled Swift objects run on.
    deployment: OsVersion,
    /// Whether this is a Mac Catalyst target. It compiles against the macOS
    /// SDK, whose `UIKit` lives under `System/iOSSupport`, outside `swiftc`'s
    /// default framework search path.
    mac_catalyst: bool,
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
impl SwiftTarget {
    fn from_cargo_env() -> Self {
        let rust_target = cargo_env("TARGET");
        let mac_catalyst = rust_target.ends_with("-apple-ios-macabi");
        let (sdk, swift_triple, runtime_dir, deployment) = if rust_target.contains("ios") {
            let arch = if rust_target.contains("x86_64") {
                "x86_64"
            } else {
                "arm64"
            };
            let ios14 = OsVersion::new(14, 0);
            if mac_catalyst {
                // Mac Catalyst: the macOS SDK and runtime, with the `-macabi`
                // environment selecting the iOS API surface.
                (
                    "macosx",
                    format!("{arch}-apple-ios14.0-macabi"),
                    "macosx",
                    ios14,
                )
            } else if rust_target.contains("ios-sim") {
                (
                    "iphonesimulator",
                    format!("{arch}-apple-ios14.0-simulator"),
                    "iphonesimulator",
                    ios14,
                )
            } else {
                (
                    "iphoneos",
                    format!("{arch}-apple-ios14.0"),
                    "iphoneos",
                    ios14,
                )
            }
        } else {
            let arch = if rust_target.contains("aarch64") || rust_target.contains("arm64") {
                "arm64"
            } else {
                "x86_64"
            };
            (
                "macosx",
                format!("{arch}-apple-macos12.3"),
                "macosx",
                OsVersion::new(12, 3),
            )
        };
        Self {
            rust_target,
            sdk,
            swift_triple,
            runtime_dir,
            deployment,
            mac_catalyst,
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
    assert_deployment_floor(&target);
    let sdk_path = target.sdk_path();
    let obj_file = out_dir.join(format!("{module_name}.o"));
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
        .arg(module_name)
        .arg(&combined_swift)
        .arg("-target")
        .arg(&target.swift_triple);
    if target.mac_catalyst {
        swiftc.arg("-F").arg(format!(
            "{sdk_path}/System/iOSSupport/System/Library/Frameworks"
        ));
    }
    if has_ios26_background_task_apis(&target) {
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

#[cfg(test)]
mod tests {
    use super::OsVersion;

    #[test]
    fn parses_and_orders_os_versions() {
        assert_eq!(
            OsVersion::parse("12.3"),
            Some(OsVersion {
                major: 12,
                minor: 3,
                patch: 0
            })
        );
        assert_eq!(
            OsVersion::parse("10.14.4"),
            Some(OsVersion {
                major: 10,
                minor: 14,
                patch: 4
            })
        );
        assert!(OsVersion::new(12, 3) > OsVersion::parse("12.0").unwrap());
        assert!(OsVersion::new(14, 0) < OsVersion::parse("15.0").unwrap());
        assert_eq!(OsVersion::parse("x"), None);
    }
}
