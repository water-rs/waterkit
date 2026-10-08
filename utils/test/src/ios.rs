//! The iOS runner.
//!
//! It builds the harness's Rust library for the destination, builds and signs
//! the `SwiftUI` app around it with `xcodebuild`, installs and launches the app,
//! and reads back the structured report the app writes into its data
//! container.
//!
//! The report travels as a file, not over the console. The app writes it
//! atomically to `Documents/waterkit-test-reports/<run-id>.json`, under a run
//! ID the runner chose for this launch. A file that comes back is therefore
//! complete and belongs to this run. Console output has neither property: it
//! is a byte stream that a dropped connection or an unflushed buffer can cut
//! short, and it carries no proof of which launch produced it. The console is
//! still bridged to the terminal so a run can be followed live, and the launch
//! uses it to wait for the app to exit.

use std::ffi::OsString;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cargo_metadata::{Message, TargetKind};
use eyre::{Context, Result};
use owo_colors::OwoColorize;
use tracing::{info, warn};
use waterkit_test_report::from_json;

use crate::{ensure_report_success, harness_feature, run_with_timeout, workspace_root};

/// Bundle identifier of the harness app, as `Info.plist` and the Xcode project
/// declare it.
const BUNDLE_ID: &str = "com.waterkit.test";

/// The Xcode project that builds the harness app, relative to the workspace
/// root.
const PROJECT: &str = "tests/ios/app/WaterKitTest.xcodeproj";

/// The project's app target.
const TARGET: &str = "WaterKitTest";

/// Manifest of the Rust library the app links, relative to the workspace root.
const HARNESS_MANIFEST: &str = "tests/ios/rust/Cargo.toml";

/// How long one launch may take, from the launch request until the app exits
/// after writing its report.
///
/// The camera case dominates a device run: it may wait up to a minute for
/// someone to answer the camera access prompt, then streams from every camera
/// in turn with 15 s for each (an iPhone 16 Pro has four).
const RUN_TIMEOUT: Duration = Duration::from_secs(300);

/// Arguments of `waterkit-test ios`.
#[derive(clap::Args)]
pub struct IosArgs {
    /// Path to the crate to run
    crate_path: PathBuf,

    #[command(flatten)]
    device: Option<DeviceArgs>,
}

/// Selects a physical device instead of the booted simulator.
#[derive(clap::Args)]
struct DeviceArgs {
    /// Run on this paired physical device instead of the booted simulator: its
    /// UDID or name, as `xcrun devicectl list devices` shows it
    #[arg(long = "device", required = false)]
    device: String,

    /// Development team ID that signs the app. `xcodebuild` provisions the app
    /// automatically for this team, so Xcode must be signed in to an account
    /// that belongs to it
    #[arg(long, required = false)]
    team: String,

    /// SSH destination of the Mac the device is attached to, when that is not
    /// this Mac. The app is built and signed here, then installed and run
    /// through `devicectl` on that Mac
    #[arg(long)]
    ssh: Option<String>,
}

/// Runs the harness for one crate on the booted simulator or on a device.
pub fn run(args: IosArgs) -> Result<()> {
    info!("{}", "Preparing iOS test environment...".green().bold());

    let root = workspace_root();
    let feature = harness_feature(&args.crate_path)?;

    match args.device {
        None => run_on(&root, feature, &Simulator),
        Some(DeviceArgs {
            device,
            team,
            ssh: None,
        }) => {
            let work_dir = root.join("target/waterkit-test-ios/device");
            std::fs::create_dir_all(&work_dir)
                .with_context(|| format!("Failed to create {}", work_dir.display()))?;
            run_on(
                &root,
                feature,
                &Device {
                    id: device,
                    team,
                    host: LocalHost { work_dir },
                },
            )
        }
        Some(DeviceArgs {
            device,
            team,
            ssh: Some(destination),
        }) => run_on(
            &root,
            feature,
            &Device {
                id: device,
                team,
                host: SshHost::connect(destination)?,
            },
        ),
    }
}

fn run_on(root: &Path, feature: &str, destination: &impl Destination) -> Result<()> {
    let library = build_library(root, destination.rust_target(), feature)?;
    let app = build_app(root, destination, &library)?;

    info!("{}", "Installing app...".yellow().bold());
    destination.install(&app)?;
    destination.grant_permissions(feature)?;

    let run_id = format!(
        "{:x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("System clock is before the Unix epoch")?
            .as_nanos()
    );
    info!(
        "{}",
        format!("Launching app (run {run_id})...").green().bold()
    );
    let report_json = destination.run(&run_id)?;
    let report = from_json(&report_json).context("Failed to parse the iOS test report")?;
    ensure_report_success(&report)
}

/// Where the app writes the report of run `run_id`, relative to its data
/// container.
fn report_path(run_id: &str) -> String {
    format!("Documents/waterkit-test-reports/{run_id}.json")
}

/// The harness's static library and the native link flags rustc reports for
/// it.
struct RustLibrary {
    /// The archive cargo built.
    archive: PathBuf,
    /// rustc's `native-static-libs` flags: every system library and framework
    /// the archive's crates declare with `#[link]`, which a static archive
    /// cannot carry to its consumer's link.
    link_flags: String,
}

/// Builds the harness's static library for `rust_target` and returns it with
/// the native link flags rustc reports for it.
fn build_library(root: &Path, rust_target: &str, feature: &str) -> Result<RustLibrary> {
    info!(
        "{}",
        format!("Building iOS test library for {rust_target}...")
            .yellow()
            .bold()
    );
    let manifest = root.join(HARNESS_MANIFEST);
    // `cargo rustc` passes `--print=native-static-libs` to the harness crate
    // alone, and Cargo replays the resulting note on a fresh unit as well as
    // on a rebuilt one. Diagnostics stay JSON on stdout, where the note can be
    // read; every other diagnostic is logged as rustc rendered it.
    let mut child = Command::new("cargo")
        .current_dir(root)
        .args([
            "rustc",
            "--lib",
            "--message-format=json-diagnostic-rendered-ansi",
        ])
        .arg("--manifest-path")
        .arg(&manifest)
        .args(["--target", rust_target, "--features", feature])
        .args(["--", "--print=native-static-libs"])
        .stdout(Stdio::piped())
        .spawn()
        .context("Failed to run cargo rustc")?;

    let stdout = child
        .stdout
        .take()
        .expect("cargo's stdout was requested as a pipe");
    let mut archive = None;
    let mut link_flags = None;
    for message in Message::parse_stream(BufReader::new(stdout)) {
        match message.context("Failed to read cargo's build messages")? {
            Message::CompilerArtifact(artifact)
                if artifact.manifest_path.as_std_path() == manifest
                    && artifact.target.is_kind(TargetKind::StaticLib) =>
            {
                archive = artifact
                    .filenames
                    .into_iter()
                    .find(|file| file.extension() == Some("a"));
            }
            Message::CompilerMessage(message) => {
                if let Some(flags) = message.message.message.strip_prefix("native-static-libs: ") {
                    link_flags = Some(app_link_flags(flags));
                } else if let Some(rendered) = message.message.rendered {
                    warn!("{}", rendered.trim_end());
                }
            }
            _ => {}
        }
    }

    let status = child.wait().context("Failed to wait for cargo rustc")?;
    if !status.success() {
        eyre::bail!("iOS test library build failed for {rust_target}");
    }
    let archive = archive
        .map(Into::into)
        .ok_or_else(|| eyre::eyre!("cargo built no static library from {}", manifest.display()))?;
    let link_flags = link_flags.ok_or_else(|| {
        eyre::eyre!(
            "rustc reported no native-static-libs for {}",
            manifest.display()
        )
    })?;
    Ok(RustLibrary {
        archive,
        link_flags,
    })
}

/// The app link's share of rustc's `native-static-libs` flags, each library
/// once in first-seen order. `clang_rt.*` is left out: rustc names it without
/// the toolchain directory that holds it, and the clang driver `xcodebuild`
/// links the app with adds the platform's compiler runtime itself.
fn app_link_flags(flags: &str) -> String {
    let mut words = flags.split_whitespace();
    let mut kept: Vec<String> = Vec::new();
    while let Some(word) = words.next() {
        let flag = if word == "-framework" {
            let name = words
                .next()
                .expect("rustc names a framework after every -framework");
            format!("-framework {name}")
        } else if word.starts_with("-lclang_rt.") {
            continue;
        } else {
            word.to_owned()
        };
        if !kept.contains(&flag) {
            kept.push(flag);
        }
    }
    kept.join(" ")
}

/// Builds the app around `library` with `xcodebuild`, signed for
/// `destination`, and returns the `.app` bundle.
///
/// Every run builds from clean: the project links the library through a build
/// setting, which Xcode does not track as a link input, so an incremental
/// build could keep a binary linked against an older library.
fn build_app(
    root: &Path,
    destination: &impl Destination,
    library: &RustLibrary,
) -> Result<PathBuf> {
    info!("{}", "Building and signing the app...".yellow().bold());
    let products = library
        .archive
        .parent()
        .expect("a cargo artifact path has a parent directory")
        .join("WaterKitTest-xcode");

    let mut settings: Vec<OsString> = Vec::new();
    for (name, value) in [
        ("SYMROOT", products.join("Build").into_os_string()),
        ("OBJROOT", products.join("Intermediates").into_os_string()),
        (
            "CONFIGURATION_BUILD_DIR",
            products.join("Products").into_os_string(),
        ),
        (
            "WATERKIT_RUST_LIBRARY",
            library.archive.as_os_str().to_owned(),
        ),
        (
            "WATERKIT_RUST_LINK_FLAGS",
            OsString::from(&library.link_flags),
        ),
    ] {
        let mut setting = OsString::from(format!("{name}="));
        setting.push(value);
        settings.push(setting);
    }

    let status = Command::new("xcodebuild")
        .current_dir(root)
        .arg("-quiet")
        .arg("-project")
        .arg(root.join(PROJECT))
        .args(["-target", TARGET, "-configuration", "Debug"])
        .args(["-sdk", destination.sdk()])
        .args(destination.signing_arguments())
        .args(settings)
        .args(["clean", "build"])
        .status()
        .context("Failed to run xcodebuild")?;
    if !status.success() {
        eyre::bail!("xcodebuild failed to build the harness app");
    }

    let app = products.join("Products/WaterKitTest.app");
    if !app.is_dir() {
        eyre::bail!(
            "xcodebuild reported success but {} is missing",
            app.display()
        );
    }
    Ok(app)
}

/// Where the harness app runs.
trait Destination {
    /// The Rust target triple the harness library is built for.
    fn rust_target(&self) -> &'static str;

    /// The SDK `xcodebuild` builds the app against.
    fn sdk(&self) -> &'static str;

    /// The `xcodebuild` arguments that sign the app for this destination.
    fn signing_arguments(&self) -> Vec<String>;

    /// Installs the built app.
    fn install(&self, app: &Path) -> Result<()>;

    /// Grants, before launch, the permissions the cases of `feature` need.
    fn grant_permissions(&self, feature: &str) -> Result<()>;

    /// Launches the app for run `run_id`, waits for it to exit and returns the
    /// report it wrote.
    fn run(&self, run_id: &str) -> Result<String>;
}

/// The booted iOS simulator.
struct Simulator;

/// The `simctl` device selector for the booted simulator.
const BOOTED: &str = "booted";

impl Destination for Simulator {
    fn rust_target(&self) -> &'static str {
        "aarch64-apple-ios-sim"
    }

    fn sdk(&self) -> &'static str {
        "iphonesimulator"
    }

    /// None: Xcode signs a simulator build ad hoc and embeds its entitlements
    /// in the binary's `__entitlements` section, where the simulator reads
    /// them, rather than in the signature.
    fn signing_arguments(&self) -> Vec<String> {
        Vec::new()
    }

    fn install(&self, app: &Path) -> Result<()> {
        let status = Command::new("xcrun")
            .args(["simctl", "install", BOOTED])
            .arg(app)
            .status()
            .context("Failed to run simctl install")?;
        if !status.success() {
            eyre::bail!("Installation failed (ensure a simulator is booted)");
        }
        Ok(())
    }

    /// Grants the TCC permissions the harness can set without the system
    /// prompt once the app is installed, then plants a deterministic simulated
    /// location so `Location::get()` has a fix to return. Notification
    /// authorization is not a `simctl privacy` service, so that case skips
    /// instead.
    fn grant_permissions(&self, feature: &str) -> Result<()> {
        if !matches!(feature, "full" | "location" | "permission") {
            return Ok(());
        }

        for service in ["location", "location-always"] {
            let status = Command::new("xcrun")
                .args(["simctl", "privacy", BOOTED, "grant", service, BUNDLE_ID])
                .status()
                .context("Failed to grant simulator privacy permission")?;
            if !status.success() {
                eyre::bail!("simctl privacy grant {service} failed");
            }
        }

        let status = Command::new("xcrun")
            .args(["simctl", "location", BOOTED, "set", "37.3349,-122.0090"])
            .status()
            .context("Failed to set simulated location")?;
        if !status.success() {
            eyre::bail!("simctl location set failed");
        }

        Ok(())
    }

    fn run(&self, run_id: &str) -> Result<String> {
        let mut launch = Command::new("xcrun");
        launch.args([
            "simctl",
            "launch",
            "--console",
            BOOTED,
            BUNDLE_ID,
            "--waterkit-run-test",
            run_id,
        ]);
        let output = run_with_timeout(launch, RUN_TIMEOUT, "run the harness app on the simulator")?;
        if !output.status.success() {
            eyre::bail!("simctl launch failed with {}", output.status);
        }

        let container = Command::new("xcrun")
            .args(["simctl", "get_app_container", BOOTED, BUNDLE_ID, "data"])
            .output()
            .context("Failed to query the iOS app data container")?;
        if !container.status.success() {
            eyre::bail!(
                "simctl get_app_container failed: {}",
                String::from_utf8_lossy(&container.stderr).trim()
            );
        }
        let container = String::from_utf8(container.stdout)
            .context("iOS app data container path was not valid UTF-8")?;
        let report = PathBuf::from(container.trim()).join(report_path(run_id));
        std::fs::read_to_string(&report)
            .with_context(|| format!("The iOS app did not write {}", report.display()))
    }
}

/// A paired physical device, reached through `devicectl` on `host`.
struct Device<H> {
    /// UDID or name of the device.
    id: String,
    /// Development team that signs the app.
    team: String,
    /// The Mac the device is attached to.
    host: H,
}

impl<H: DeviceHost> Device<H> {
    /// Runs `xcrun devicectl <args>` on the device's Mac, bounded by
    /// `timeout`, with its output bridged to this terminal.
    fn devicectl(&self, args: &[&str], timeout: Duration, description: &str) -> Result<()> {
        let mut command = self.host.xcrun(&[&["devicectl"], args].concat())?;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        let output = run_with_timeout(command, timeout, description)?;
        if !output.status.success() {
            eyre::bail!(
                "devicectl could not {description} (exit {}); its error is printed above",
                output.status
            );
        }
        Ok(())
    }
}

/// How long a `devicectl` step other than the harness run may take.
const DEVICECTL_TIMEOUT: Duration = Duration::from_secs(120);

impl<H: DeviceHost> Destination for Device<H> {
    fn rust_target(&self) -> &'static str {
        "aarch64-apple-ios"
    }

    fn sdk(&self) -> &'static str {
        "iphoneos"
    }

    /// Automatic provisioning: `xcodebuild` registers the App ID, creates or
    /// renews the development profile for the team, and signs with it.
    fn signing_arguments(&self) -> Vec<String> {
        vec![
            "-allowProvisioningUpdates".to_owned(),
            format!("DEVELOPMENT_TEAM={}", self.team),
        ]
    }

    fn install(&self, app: &Path) -> Result<()> {
        let app = self.host.stage_app(app)?;
        self.devicectl(
            &["device", "install", "app", "--device", &self.id, &app],
            DEVICECTL_TIMEOUT,
            "install the harness app on the device",
        )
    }

    /// Nothing can be granted from the host: iOS has no `simctl privacy` for a
    /// device, and its privacy permissions come only from the system prompt,
    /// answered on the device. The cases request what they need and report a
    /// denied or unanswered prompt; the answer persists until the app is
    /// deleted, so it is needed once per device.
    fn grant_permissions(&self, feature: &str) -> Result<()> {
        if matches!(feature, "full" | "camera") {
            info!(
                "Camera access on a device is granted only by answering the system prompt on the \
                 device. If this app has not been allowed yet, the prompt appears during the run: \
                 tap Allow on the device."
            );
        }
        Ok(())
    }

    fn run(&self, run_id: &str) -> Result<String> {
        // `--console` bridges the app's output and keeps devicectl running
        // until the app exits, which is the run's completion signal.
        self.devicectl(
            &[
                "device",
                "process",
                "launch",
                "--device",
                &self.id,
                "--terminate-existing",
                "--console",
                BUNDLE_ID,
                "--waterkit-run-test",
                run_id,
            ],
            RUN_TIMEOUT,
            "run the harness app on the device",
        )?;

        let destination = self.host.report_destination(run_id);
        self.devicectl(
            &[
                "device",
                "copy",
                "from",
                "--device",
                &self.id,
                "--domain-type",
                "appDataContainer",
                "--domain-identifier",
                BUNDLE_ID,
                "--source",
                &report_path(run_id),
                "--destination",
                &destination,
            ],
            DEVICECTL_TIMEOUT,
            "copy the test report out of the app container",
        )?;
        self.host.read(&destination)
    }
}

/// The Mac a device is attached to.
trait DeviceHost {
    /// A command that runs `xcrun <args>` on that Mac.
    fn xcrun(&self, args: &[&str]) -> Result<Command>;

    /// Makes the built `app` available on that Mac and returns its path there.
    fn stage_app(&self, app: &Path) -> Result<String>;

    /// The path on that Mac where the report of run `run_id` is copied to.
    fn report_destination(&self, run_id: &str) -> String;

    /// Reads a text file on that Mac.
    fn read(&self, path: &str) -> Result<String>;
}

/// The device is attached to this Mac.
struct LocalHost {
    /// Directory for the reports copied off the device.
    work_dir: PathBuf,
}

impl DeviceHost for LocalHost {
    fn xcrun(&self, args: &[&str]) -> Result<Command> {
        let mut command = Command::new("xcrun");
        command.args(args);
        Ok(command)
    }

    fn stage_app(&self, app: &Path) -> Result<String> {
        app.to_str()
            .map(str::to_owned)
            .ok_or_else(|| eyre::eyre!("App path {} is not valid UTF-8", app.display()))
    }

    fn report_destination(&self, run_id: &str) -> String {
        self.work_dir
            .join(format!("{run_id}.json"))
            .to_string_lossy()
            .into_owned()
    }

    fn read(&self, path: &str) -> Result<String> {
        std::fs::read_to_string(path).with_context(|| format!("Failed to read {path}"))
    }
}

/// The device is attached to another Mac, reached over SSH.
///
/// SSH hands its command to the remote login shell as one string, so every
/// argument is single-quoted (see [`remote_word`]). A temporary directory on
/// that Mac holds the staged app and the copied report, and is removed when
/// the run ends.
struct SshHost {
    destination: String,
    work_dir: String,
}

impl SshHost {
    fn connect(destination: String) -> Result<Self> {
        if destination.starts_with('-') {
            eyre::bail!("SSH destination {destination:?} would be read as an ssh option");
        }
        let output = Command::new("ssh")
            .arg(&destination)
            .args(["mktemp", "-d", "-t", "waterkit-test"])
            .output()
            .with_context(|| format!("Failed to run ssh {destination}"))?;
        if !output.status.success() {
            eyre::bail!(
                "Could not create a work directory on {destination}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let work_dir = String::from_utf8(output.stdout)
            .context("Remote mktemp printed a path that is not valid UTF-8")?
            .trim()
            .to_owned();
        Ok(Self {
            destination,
            work_dir,
        })
    }

    fn command(&self, words: &[&str]) -> Result<Command> {
        let mut command = Command::new("ssh");
        command.arg(&self.destination);
        for word in words {
            command.arg(remote_word(word)?);
        }
        Ok(command)
    }
}

impl DeviceHost for SshHost {
    fn xcrun(&self, args: &[&str]) -> Result<Command> {
        self.command(&[&["xcrun"], args].concat())
    }

    fn stage_app(&self, app: &Path) -> Result<String> {
        let name = app
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| eyre::eyre!("App path {} has no UTF-8 file name", app.display()))?;
        // scp addresses the remote path itself (over SFTP), so the path goes
        // unquoted; it is the `mktemp` path, which contains no shell syntax.
        let target = format!("{}:{}", self.destination, self.work_dir);
        let status = Command::new("scp")
            .args(["-rq"])
            .arg(app)
            .arg(target)
            .status()
            .context("Failed to run scp")?;
        if !status.success() {
            eyre::bail!(
                "scp could not copy {} to {}",
                app.display(),
                self.destination
            );
        }
        Ok(format!("{}/{name}", self.work_dir))
    }

    fn report_destination(&self, run_id: &str) -> String {
        format!("{}/{run_id}.json", self.work_dir)
    }

    fn read(&self, path: &str) -> Result<String> {
        let output = self
            .command(&["cat", path])?
            .output()
            .context("Failed to run ssh")?;
        if !output.status.success() {
            eyre::bail!(
                "Could not read {path} on {}: {}",
                self.destination,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        String::from_utf8(output.stdout).with_context(|| format!("{path} is not valid UTF-8"))
    }
}

impl Drop for SshHost {
    fn drop(&mut self) {
        let removed = self
            .command(&["rm", "-rf", &self.work_dir])
            .and_then(|mut command| command.status().context("Failed to run ssh"));
        match removed {
            Ok(status) if status.success() => {}
            Ok(status) => warn!(
                "Removing {} on {} failed with {status}",
                self.work_dir, self.destination
            ),
            Err(error) => warn!(
                "Removing {} on {} failed: {error:#}",
                self.work_dir, self.destination
            ),
        }
    }
}

/// Quotes `word` for the remote login shell.
///
/// Inside single quotes POSIX shells take every character literally, and so
/// does fish except for `\'` and `\\`. A word without `'` or `\` therefore
/// means the same to every shell a Mac may log in with; any other word is
/// rejected rather than quoted for one shell and misread by another.
fn remote_word(word: &str) -> Result<String> {
    if word.contains(['\'', '\\']) {
        eyre::bail!("{word:?} cannot be passed over SSH: it contains a quote or a backslash");
    }
    Ok(format!("'{word}'"))
}

#[cfg(test)]
mod tests {
    use super::{app_link_flags, remote_word};

    #[test]
    fn quotes_remote_words() {
        assert_eq!(
            remote_word("Lexo\u{2019}s iPhone").unwrap(),
            "'Lexo\u{2019}s iPhone'"
        );
    }

    #[test]
    fn rejects_words_shells_quote_differently() {
        assert!(remote_word("it's").is_err());
        assert!(remote_word(r"a\b").is_err());
    }

    #[test]
    fn app_link_flags_drop_the_compiler_runtime_and_repeats() {
        assert_eq!(
            app_link_flags(
                "-lclang_rt.iossim -framework Foundation -lSystem -framework UIKit \
                 -lclang_rt.iossim -framework Foundation -lSystem -lobjc"
            ),
            "-framework Foundation -lSystem -framework UIKit -lobjc"
        );
    }
}
