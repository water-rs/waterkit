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

/// A Swift module compiled header-free into a static archive and a
/// `.swiftmodule`, for callers that own their Swift/Rust ABI without
/// `swift-bridge`.
///
/// Every source path is crate-relative and resolved against
/// `CARGO_MANIFEST_DIR`; an absolute path passes through unchanged. The
/// module name is what `swiftc -module-name` writes — callers normalize
/// their crate name themselves (for example `pkg_name.replace('-', "_")`).
#[derive(Debug, Clone)]
pub struct SwiftModule {
    module_name: String,
    sources: Vec<PathBuf>,
    frameworks: Vec<String>,
    compiler_args: Vec<std::ffi::OsString>,
    bridging_header: Option<PathBuf>,
}

/// The products of [`SwiftModule::compile`]: the static archive Cargo links
/// and the module file a Swift consumer `import`s.
#[derive(Debug)]
pub struct SwiftArtifacts {
    archive: PathBuf,
    module: PathBuf,
}

impl SwiftArtifacts {
    /// `OUT_DIR/lib<module>.a`, the archive the crate's own targets link.
    #[must_use]
    pub fn archive(&self) -> &Path {
        &self.archive
    }

    /// `OUT_DIR/<module>.swiftmodule` — add its directory to a Swift
    /// consumer's `-I` path to `import` the module.
    #[must_use]
    pub fn module(&self) -> &Path {
        &self.module
    }
}

impl SwiftModule {
    /// A module builder named `name`, with no sources, frameworks or
    /// compiler args yet.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            module_name: name.into(),
            sources: Vec::new(),
            frameworks: Vec::new(),
            compiler_args: Vec::new(),
            bridging_header: None,
        }
    }

    /// Adds a Swift source file, as a crate-relative path. Sources compile
    /// as separate files in one whole-module invocation, so file-scoped
    /// `private` declarations keep their file boundary.
    #[must_use]
    pub fn swift_source(mut self, path: impl Into<PathBuf>) -> Self {
        self.sources.push(path.into());
        self
    }

    /// Adds an Apple framework the module's Swift code links against.
    /// Frameworks are deduplicated across every caller.
    #[must_use]
    pub fn framework(mut self, name: impl Into<String>) -> Self {
        self.frameworks.push(name.into());
        self
    }

    /// Passes one raw `swiftc` argument, verbatim as one argv entry — no
    /// whitespace splitting, no validation; the caller owns its options.
    #[must_use]
    pub fn compiler_arg(mut self, arg: impl Into<std::ffi::OsString>) -> Self {
        self.compiler_args.push(arg.into());
        self
    }

    /// A private Objective-C bridging header `swiftc` sees through
    /// `-import-objc-header`, used by `SwiftBridges` for its generated
    /// C glue; header-free callers leave this unset.
    #[cfg(feature = "swift-bridge")]
    pub(crate) fn bridging_header(mut self, path: impl Into<PathBuf>) -> Self {
        self.bridging_header = Some(path.into());
        self
    }
}

/// One `#[swift_bridge::bridge]` module of a crate and the Swift sources and
/// frameworks that implement its `extern "Swift"` side.
///
/// The module and every source are crate-relative paths, resolved against
/// `CARGO_MANIFEST_DIR` by [`SwiftBridges`].
#[cfg(feature = "swift-bridge")]
#[derive(Debug, Clone)]
pub struct SwiftBridge {
    module: PathBuf,
    sources: Vec<PathBuf>,
    frameworks: Vec<String>,
}

#[cfg(feature = "swift-bridge")]
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
#[cfg(feature = "swift-bridge")]
#[derive(Debug, Clone, Default)]
pub struct SwiftBridges {
    bridges: Vec<SwiftBridge>,
}

#[cfg(feature = "swift-bridge")]
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

#[cfg(all(feature = "swift-bridge", any(target_os = "ios", target_os = "macos")))]
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

        // The generated C headers reach the Swift module through a private
        // bridging header — it is glue for this pipeline, not a public
        // `SwiftModule` input.
        let bridging_header = out_dir.join("Bridging-Header.h");
        std::fs::write(
            &bridging_header,
            format!(
                "#include \"{}\"\n#include \"{}\"\n",
                out_dir.join("SwiftBridgeCore.h").display(),
                out_dir.join(format!("{pkg_name}/{pkg_name}.h")).display(),
            ),
        )
        .expect("Failed to write bridging header");

        let mut module = SwiftModule::new(pkg_name.replace('-', "_"))
            .swift_source(out_dir.join("SwiftBridgeCore.swift"))
            .swift_source(out_dir.join(format!("{pkg_name}/{pkg_name}.swift")))
            .bridging_header(&bridging_header);
        for bridge in &self.bridges {
            for source in &bridge.sources {
                module = module.swift_source(manifest_dir.join(source));
            }
            for framework in &bridge.frameworks {
                module = module.framework(framework.clone());
            }
        }
        module.compile();
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
#[cfg(all(
    feature = "swift-bridge",
    not(any(target_os = "ios", target_os = "macos"))
))]
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

impl SwiftModule {
    /// The empty-source gate `compile` and the unit tests share.
    fn assert_sources(&self) {
        assert!(
            !self.sources.is_empty(),
            "SwiftModule::compile() for module {} has no Swift sources; \
             add each one with .swift_source(..)",
            self.module_name,
        );
    }
}

/// The `swiftc` arguments [`SwiftModule::compile`] runs: one object and one
/// module out of one whole-module invocation, every source its own argv
/// entry so `private` keeps its file boundary.
#[cfg(any(target_os = "ios", target_os = "macos"))]
fn swiftc_args(
    module: &SwiftModule,
    sdk_path: &str,
    target: &SwiftTarget,
    obj: &Path,
    swiftmodule: &Path,
    sources: &[PathBuf],
) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = vec![
        "-emit-object".into(),
        "-whole-module-optimization".into(),
        "-emit-module".into(),
        "-emit-module-path".into(),
        swiftmodule.into(),
        "-o".into(),
        obj.into(),
        "-sdk".into(),
        sdk_path.into(),
    ];
    if let Some(header) = &module.bridging_header {
        args.push("-import-objc-header".into());
        args.push(header.into());
    }
    args.extend([
        "-parse-as-library".into(),
        "-module-name".into(),
        module.module_name.clone().into(),
        "-target".into(),
        target.swift_triple.clone().into(),
    ]);
    if target.mac_catalyst {
        args.push("-F".into());
        args.push(format!("{sdk_path}/System/iOSSupport/System/Library/Frameworks").into());
    }
    if has_ios26_background_task_apis(target) {
        args.push("-D".into());
        args.push("WATERKIT_HAS_IOS26_BACKGROUND_TASKS".into());
    }
    args.extend(module.compiler_args.iter().cloned());
    args.extend(sources.iter().map(|source| source.clone().into()));
    args
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
impl SwiftModule {
    /// Compiles every declared source into `OUT_DIR` as the module,
    /// archives the object as `lib<name>.a`, links that archive, the Swift
    /// runtime and each declared framework into the crate, and emits
    /// `rerun-if-changed` for `build.rs`, every source and the private
    /// bridging header when one was supplied.
    ///
    /// # Panics
    ///
    /// Panics when it does not run inside a Cargo build script, when no
    /// sources were declared (naming the module), or when the Swift
    /// toolchain or `ar` fails — its error carries the full `swiftc` argv
    /// and output.
    pub fn compile(self) -> SwiftArtifacts {
        let manifest_dir = PathBuf::from(cargo_env("CARGO_MANIFEST_DIR"));
        let out_dir = PathBuf::from(cargo_env("OUT_DIR"));
        self.assert_sources();

        println!("cargo:rerun-if-changed=build.rs");
        let sources: Vec<PathBuf> = self
            .sources
            .iter()
            .map(|source| manifest_dir.join(source))
            .collect();
        for source in &sources {
            println!("cargo:rerun-if-changed={}", source.display());
        }
        if let Some(header) = &self.bridging_header {
            println!("cargo:rerun-if-changed={}", header.display());
        }

        let target = SwiftTarget::from_cargo_env();
        assert_deployment_floor(&target);
        let sdk_path = target.sdk_path();
        let obj = out_dir.join(format!("{}.o", self.module_name));
        let swiftmodule = out_dir.join(format!("{}.swiftmodule", self.module_name));
        let args = swiftc_args(&self, &sdk_path, &target, &obj, &swiftmodule, &sources);
        let output = std::process::Command::new("swiftc")
            .args(&args)
            .output()
            .expect("Failed to run swiftc");
        assert!(
            output.status.success(),
            "Swift compilation of module {} failed (swiftc {:?}):\nstdout: {}\nstderr: {}",
            self.module_name,
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );

        let archive = out_dir.join(format!("lib{}.a", self.module_name));
        let ar_status = std::process::Command::new("ar")
            .arg("rcs")
            .arg(&archive)
            .arg(&obj)
            .status()
            .expect("Failed to run ar");
        assert!(ar_status.success(), "ar failed");

        println!("cargo:rustc-link-search=native={}", out_dir.display());
        println!("cargo:rustc-link-lib=static={}", self.module_name);
        let frameworks: BTreeSet<&str> = self.frameworks.iter().map(String::as_str).collect();
        for framework in frameworks {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
        link_swift_runtime(target.runtime_dir);
        SwiftArtifacts {
            archive,
            module: swiftmodule,
        }
    }
}

/// Swift is compiled with the Xcode toolchain, which only a macOS host has;
/// the module builder exists everywhere so a non-Apple build host fails
/// clearly at `compile`, not at name resolution.
#[cfg(not(any(target_os = "ios", target_os = "macos")))]
impl SwiftModule {
    /// Compiling Swift requires the Xcode toolchain.
    ///
    /// # Panics
    ///
    /// Always: a build script reaches this only when it targets an Apple
    /// platform from a host that cannot build for one.
    pub fn compile(self) -> SwiftArtifacts {
        self.assert_sources();
        panic!(
            "compiling the Swift module {} ({} sources, {} frameworks, \
             {} compiler args, bridging header {}) requires a macOS host with Xcode",
            self.module_name,
            self.sources.len(),
            self.frameworks.len(),
            self.compiler_args.len(),
            self.bridging_header
                .as_ref()
                .map(|header| header.display().to_string())
                .unwrap_or_else(|| "none".to_string()),
        );
    }
}

#[cfg(all(test, any(target_os = "ios", target_os = "macos")))]
mod module_tests {
    use super::*;
    use std::ffi::OsString;

    fn target() -> SwiftTarget {
        SwiftTarget {
            rust_target: "aarch64-apple-ios-sim".into(),
            sdk: "iphonesimulator",
            swift_triple: "arm64-apple-ios14.0-simulator".into(),
            runtime_dir: "iphonesimulator",
            deployment: OsVersion::new(14, 0),
            mac_catalyst: false,
        }
    }

    fn args(module: &SwiftModule) -> Vec<OsString> {
        swiftc_args(
            module,
            "/sdk",
            &target(),
            Path::new("/out/M.o"),
            Path::new("/out/M.swiftmodule"),
            &module.sources,
        )
    }

    #[test]
    fn header_free_module_carries_sources_args_and_no_header() {
        let module = SwiftModule::new("Probe")
            .swift_source("a/First.swift")
            .swift_source("b/Second.swift")
            .compiler_arg("-D")
            .compiler_arg("PROBE")
            .compiler_arg("-enable-experimental-feature")
            .compiler_arg("Extern");
        let args = args(&module);
        // Every source is its own argv entry — no concatenated file.
        let tail: Vec<&std::ffi::OsStr> = args
            .iter()
            .skip_while(|arg| arg.as_os_str() != "a/First.swift")
            .map(OsString::as_os_str)
            .collect();
        assert_eq!(tail, ["a/First.swift", "b/Second.swift"]);
        assert!(args.windows(2).any(|w| w == ["-D", "PROBE"]));
        assert!(
            args.windows(2)
                .any(|w| w == ["-enable-experimental-feature", "Extern"])
        );
        assert!(
            args.windows(2)
                .any(|w| w == ["-emit-module-path", "/out/M.swiftmodule"])
        );
        assert!(args.windows(2).any(|w| w == ["-o", "/out/M.o"]));
        assert!(!args.iter().any(|arg| arg == "-import-objc-header"));
        assert!(args.windows(2).any(|w| w == ["-module-name", "Probe"]));
    }

    #[cfg(feature = "swift-bridge")]
    #[test]
    fn bridging_header_joins_the_argv_and_marks_sources() {
        let module = SwiftModule::new("Pkg")
            .swift_source("SwiftBridgeCore.swift")
            .swift_source("Pkg/Pkg.swift")
            .swift_source("Feature.swift")
            .bridging_header("/out/Bridging-Header.h");
        let args = args(&module);
        assert!(
            args.windows(2)
                .any(|w| w == ["-import-objc-header", "/out/Bridging-Header.h"])
        );
        assert!(args.ends_with(&[
            OsString::from("SwiftBridgeCore.swift"),
            OsString::from("Pkg/Pkg.swift"),
            OsString::from("Feature.swift"),
        ]));
    }

    #[test]
    #[should_panic(expected = "for module EmptyModule has no Swift sources")]
    fn a_sourceless_module_fails_naming_the_module() {
        SwiftModule::new("EmptyModule").assert_sources();
    }
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
