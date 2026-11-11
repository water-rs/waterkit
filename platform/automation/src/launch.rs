// Copyright (c) 2026 Cua AI, Inc.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
//
// Derived from cua-driver (https://github.com/trycua/cua):
// libs/cua-driver/rust/crates/platform-macos/src/apps/nsworkspace.rs and
// libs/cua-driver/rust/crates/platform-macos/src/tools/launch_app.rs.

//! Launching an application in the background through `NSWorkspace`.

use core::ffi::CStr;
use core::ptr::NonNull;
use core::time::Duration;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Instant;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_app_kit::{NSRunningApplication, NSWorkspace, NSWorkspaceOpenConfiguration};
use objc2_core_graphics::{CGDirectDisplayID, CGWindowListOption, kCGNullWindowID};
use objc2_core_services::{
    AEReturnID, kAEOpenApplication, kAnyTransactionID, kAutoGenerateReturnID, kCoreEventClass,
};
use objc2_foundation::{NSAppleEventDescriptor, NSArray, NSBundle, NSError, NSString, NSURL};

use crate::accessibility::{Application, WindowLookup};
use crate::skylight::EventSymbols;
use crate::window::{self, ListedWindow};
use crate::{Error, bundle_names, placement, run_loop, topology};

/// The default for [`LaunchConfig::timeout`]: how long `LaunchServices` may
/// take to deliver a launch's completion before a wedged launch is reported.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// An application to launch, and what to hand it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchConfig {
    app: App,
    open: Vec<Open>,
    new_instance: bool,
    display: Option<CGDirectDisplayID>,
    timeout: Duration,
}

/// How the application is named.
#[derive(Debug, Clone, PartialEq, Eq)]
enum App {
    BundleId(String),
    Bundle(PathBuf),
    Name(String),
}

/// One item the application is asked to open.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Open {
    Url(String),
    File(PathBuf),
}

impl LaunchConfig {
    /// The application `LaunchServices` knows by `bundle_id`, such as
    /// `com.apple.TextEdit`.
    #[must_use]
    pub fn bundle_id(bundle_id: impl Into<String>) -> Self {
        Self::new(App::BundleId(bundle_id.into()))
    }

    /// The application bundle at `path`, such as
    /// `/System/Applications/TextEdit.app`. A relative path is resolved
    /// against the current directory.
    #[must_use]
    pub fn bundle(path: impl Into<PathBuf>) -> Self {
        Self::new(App::Bundle(path.into()))
    }

    /// The installed application known by `name`, such as `Google Chrome`
    /// or a localized name such as `Échecs`.
    ///
    /// The name is matched, without case, against every name the installed
    /// applications ship: the bundle's file name, `CFBundleDisplayName` and
    /// `CFBundleName`, and their localizations. The applications searched
    /// are those in `/Applications`, `/System/Applications`, their
    /// `Utilities` folders, `~/Applications` and Finder, one folder down
    /// included. An exact name beats a name that starts with the given one,
    /// and a name in one of the user's preferred languages beats the same
    /// match in another language. Accents and spaces are compared as
    /// shipped. A trailing `.app` is ignored.
    ///
    /// A name that matches nothing, or matches several applications equally
    /// well, fails the launch with [`Error::AppNameNotFound`] or
    /// [`Error::AppNameAmbiguous`] carrying the closest names.
    #[must_use]
    pub fn name(name: impl Into<String>) -> Self {
        Self::new(App::Name(name.into()))
    }

    const fn new(app: App) -> Self {
        Self {
            app,
            open: Vec::new(),
            new_instance: false,
            display: None,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Adds `url`, an absolute URL with a scheme, to the items the
    /// application is asked to open.
    #[must_use]
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.open.push(Open::Url(url.into()));
        self
    }

    /// Adds the file or directory at `path` to the items the application is
    /// asked to open. A relative path is resolved against the current
    /// directory.
    #[must_use]
    pub fn file(mut self, path: impl Into<PathBuf>) -> Self {
        self.open.push(Open::File(path.into()));
        self
    }

    /// Whether to start a new instance of the application even when one is
    /// running. Off by default: a running instance is reused. Either way only
    /// windows that appear after the launch are returned and placed; the
    /// windows the application already had stay where they are.
    #[must_use]
    pub const fn new_instance(mut self, new_instance: bool) -> Self {
        self.new_instance = new_instance;
        self
    }

    /// The display, such as a [`VirtualDisplay`](crate::VirtualDisplay)'s
    /// [`id`](crate::VirtualDisplay::id), to move the application's windows
    /// onto once it shows one. Unset, they stay where the system opens them.
    #[must_use]
    pub const fn display(mut self, display: CGDirectDisplayID) -> Self {
        self.display = Some(display);
        self
    }

    /// How long the whole launch may take: `LaunchServices` launching the
    /// application, the application showing a window and, with a display
    /// set, its windows arriving there. Thirty seconds by default.
    #[must_use]
    pub const fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// An application launched in the background, with the windows it showed.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct LaunchedApplication {
    pid: libc::pid_t,
    bundle_id: String,
    windows: Vec<ListedWindow>,
}

impl LaunchedApplication {
    /// Launches the application `config` names without activating it, and
    /// returns once it shows a new window: once `WindowServer` lists on
    /// screen a window the application reports through Accessibility that it
    /// did not have before the launch. With a
    /// [`display`](LaunchConfig::display) set, its windows are then moved
    /// onto that display, each centred and shrunk to fit it, and `launch`
    /// returns once `WindowServer` lists them all within it.
    ///
    /// The launch does not activate the application or add it to the recent
    /// items, and it sends the application an open-application Apple event
    /// (`aevt/oapp`), so an application launched cold or restoring its state
    /// still opens a window. See the crate documentation for how the waits
    /// service the calling thread's run loop.
    ///
    /// # Errors
    ///
    /// - [`Error::AppNotFound`] when no application has the bundle id;
    /// - [`Error::AppNameNotFound`] when no installed application ships the
    ///   name, and [`Error::AppNameAmbiguous`] when several match it
    ///   equally well, each with the closest names;
    /// - [`Error::NotAnAppBundle`] when the path names no application
    ///   bundle;
    /// - [`Error::FileNotFound`] when the bundle or a file to open does not
    ///   exist;
    /// - [`Error::InvalidUrl`] when a URL to open has no scheme;
    /// - [`Error::DisplayNotFound`] when the display is not online;
    /// - [`Error::LaunchFailed`] when `LaunchServices` reports a failure;
    /// - [`Error::LaunchTimedOut`] when it neither reports the launch nor
    ///   starts a process of the application in time;
    /// - [`Error::NoAppWindow`] when the application shows no new window in
    ///   time, as a running instance that handles the launch in a window it
    ///   already had does;
    /// - [`Error::NotPlaced`] when a window does not end up within the
    ///   display in time;
    /// - [`Error::Unavailable`] when a private function is missing;
    /// - [`Error::Platform`] when Accessibility, `CoreGraphics` or `SkyLight`
    ///   fails a call, as Accessibility does without the grant.
    pub fn launch(config: &LaunchConfig) -> Result<Self, Error> {
        let started = Instant::now();
        let remaining = || config.timeout.saturating_sub(started.elapsed());
        let app = ResolvedApp::resolve(&config.app)?;
        let open = config
            .open
            .iter()
            .map(Open::resolve)
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(display) = config.display
            && !topology::online()?.contains(&display)
        {
            return Err(Error::DisplayNotFound(display));
        }
        let symbols = EventSymbols::resolve()?;
        let lookup = WindowLookup::resolve()?;

        // The windows a running instance already has stay where they are:
        // only the ones this launch opens are returned and placed.
        let running = running_pids(&NSString::from_str(&app.bundle_id));
        let existing: Vec<u32> = window::list(CGWindowListOption::OptionAll, kCGNullWindowID)?
            .into_iter()
            .filter(|window| running.contains(&window.pid))
            .map(|window| window.id)
            .collect();
        let pid = app.open(&open, config.new_instance, config.timeout)?;
        tracing::debug!(pid, bundle_id = %app.bundle_id, "launched");
        let accessibility = Application::new(pid, &lookup, remaining())?;
        let shown = placement::shown_windows(&accessibility, &existing, remaining())?;
        if shown.is_empty() {
            return Err(Error::NoAppWindow {
                pid,
                bundle_id: app.bundle_id,
                timeout: config.timeout,
            });
        }
        if let Some(display) = config.display {
            placement::place(pid, &shown, display, remaining())?;
        }

        let listed = window::list(CGWindowListOption::OptionAll, kCGNullWindowID)?;
        let mut windows = Vec::with_capacity(shown.len());
        for shown in &shown {
            let Some(window) = listed.iter().find(|listed| listed.id == shown.id()) else {
                continue;
            };
            if let Some(application) = symbols.application_owns(window.id)? {
                windows.push(window.listed(application));
            }
        }
        Ok(Self {
            pid,
            bundle_id: app.bundle_id,
            windows,
        })
    }

    /// The application's process.
    #[must_use]
    pub const fn pid(&self) -> libc::pid_t {
        self.pid
    }

    /// The application's bundle id, as its bundle states it.
    #[must_use]
    pub fn bundle_id(&self) -> &str {
        &self.bundle_id
    }

    /// The windows the application reported through Accessibility that
    /// `WindowServer` listed on screen, as it listed them when `launch`
    /// returned, front to back.
    #[must_use]
    pub fn windows(&self) -> &[ListedWindow] {
        &self.windows
    }
}

/// An application's bundle, located.
struct ResolvedApp {
    url: Retained<NSURL>,
    bundle_id: String,
}

impl ResolvedApp {
    fn resolve(app: &App) -> Result<Self, Error> {
        let url = match app {
            App::BundleId(bundle_id) => NSWorkspace::sharedWorkspace()
                .URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle_id))
                .ok_or_else(|| Error::AppNotFound(bundle_id.clone()))?,
            App::Bundle(path) => file_url(path)?,
            App::Name(name) => file_url(&named_bundle(
                name,
                &bundle_names::app_roots(),
                &bundle_names::preferred_languages(),
            )?)?,
        };
        // The bundle states its identifier in the case LaunchServices
        // matches running applications by.
        let bundle_id = NSBundle::bundleWithURL(&url)
            .and_then(|bundle| bundle.bundleIdentifier())
            .map(|bundle_id| bundle_id.to_string());
        match (bundle_id, app) {
            (Some(bundle_id), _) => Ok(Self { url, bundle_id }),
            (None, App::Bundle(path)) => Err(Error::NotAnAppBundle(path.clone())),
            (None, App::BundleId(bundle_id)) => Err(Error::Platform(format!(
                "LaunchServices locates {bundle_id} at {url:?}, which has no bundle identifier"
            ))),
            (None, App::Name(name)) => Err(Error::Platform(format!(
                "the application named {name:?} is at {url:?}, which has no bundle identifier"
            ))),
        }
    }

    /// Asks `LaunchServices` to launch the application, handing it `open`,
    /// and waits for its process.
    ///
    /// `LaunchServices` can start the application yet never deliver the
    /// launch's completion to a process with no main run loop running, such
    /// as a daemon, so the wait also ends when a process of the application
    /// shows up: any once one runs, or with `new_instance` one that was not
    /// running before.
    fn open(
        &self,
        open: &[Retained<NSURL>],
        new_instance: bool,
        timeout: Duration,
    ) -> Result<libc::pid_t, Error> {
        let workspace = NSWorkspace::sharedWorkspace();
        let configuration = open_configuration(&self.bundle_id, new_instance);
        let bundle_id = NSString::from_str(&self.bundle_id);
        let before = running_pids(&bundle_id);
        let (sender, completion) = mpsc::sync_channel(1);
        let handler = RcBlock::new(
            move |application: *mut NSRunningApplication, error: *mut NSError| {
                // SAFETY: LaunchServices hands the handler a live application
                // or error, or null.
                let outcome = unsafe { completion_outcome(application, error) };
                if sender.try_send(outcome).is_err() {
                    tracing::debug!("a launch completed after its wait ended");
                }
            },
        );
        if open.is_empty() {
            workspace.openApplicationAtURL_configuration_completionHandler(
                &self.url,
                &configuration,
                Some(&handler),
            );
        } else {
            workspace.openURLs_withApplicationAtURL_configuration_completionHandler(
                &NSArray::from_retained_slice(open),
                &self.url,
                &configuration,
                Some(&handler),
            );
        }
        let signal = launch_signal(&completion, timeout, || {
            reconciled_pid(&running_pids(&bundle_id), &before, new_instance)
        })?;
        match signal {
            Some(Ok(pid)) => Ok(pid),
            Some(Err(reason)) => Err(Error::LaunchFailed {
                bundle_id: self.bundle_id.clone(),
                reason,
            }),
            None => Err(Error::LaunchTimedOut {
                bundle_id: self.bundle_id.clone(),
                timeout,
            }),
        }
    }
}

impl Open {
    fn resolve(&self) -> Result<Retained<NSURL>, Error> {
        match self {
            Self::File(path) => file_url(path),
            Self::Url(url) => NSURL::URLWithString(&NSString::from_str(url))
                .filter(|parsed| parsed.scheme().is_some())
                .ok_or_else(|| Error::InvalidUrl(url.clone())),
        }
    }
}

/// The bundle among `roots` that `name` names, or the refusal with the
/// closest names.
fn named_bundle(name: &str, roots: &[PathBuf], languages: &[String]) -> Result<PathBuf, Error> {
    bundle_names::find(name, roots, languages).map_err(|miss| {
        let name = name.to_owned();
        let suggestions = miss.suggestions;
        if miss.ambiguous {
            Error::AppNameAmbiguous { name, suggestions }
        } else {
            Error::AppNameNotFound { name, suggestions }
        }
    })
}

/// The file URL of `path`, made absolute against the current directory,
/// which must exist. The URL carries the path's bytes as they are.
fn file_url(path: &Path) -> Result<Retained<NSURL>, Error> {
    let path = std::path::absolute(path).map_err(|error| {
        Error::Platform(format!(
            "{} cannot be made absolute: {error}",
            path.display()
        ))
    })?;
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::FileNotFound(path));
        }
        Err(error) => {
            return Err(Error::Platform(format!(
                "reading {} failed: {error}",
                path.display()
            )));
        }
    };
    let representation = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| Error::Platform(format!("{} holds a NUL byte", path.display())))?;
    let representation: &CStr = &representation;
    // SAFETY: the representation is a NUL-terminated path that outlives the
    // call.
    Ok(unsafe {
        NSURL::fileURLWithFileSystemRepresentation_isDirectory_relativeToURL(
            NonNull::from(representation).cast(),
            metadata.is_dir(),
            None,
        )
    })
}

/// A background launch: not activating the application, not adding it to the
/// recent items, sending it `aevt/oapp`, and starting a new instance only
/// when asked.
fn open_configuration(
    bundle_id: &str,
    new_instance: bool,
) -> Retained<NSWorkspaceOpenConfiguration> {
    let configuration = NSWorkspaceOpenConfiguration::configuration();
    configuration.setActivates(false);
    configuration.setAddsToRecentItems(false);
    configuration.setCreatesNewApplicationInstance(new_instance);
    configuration.setAppleEvent(Some(&open_application_event(bundle_id)));
    configuration
}

/// The open-application Apple event (`aevt/oapp`) addressed to `bundle_id`,
/// after which an application opens its windows when nothing else asks it
/// to.
fn open_application_event(bundle_id: &str) -> Retained<NSAppleEventDescriptor> {
    let target =
        NSAppleEventDescriptor::descriptorWithBundleIdentifier(&NSString::from_str(bundle_id));
    let return_id = AEReturnID::try_from(kAutoGenerateReturnID)
        .expect("kAutoGenerateReturnID is an AEReturnID");
    NSAppleEventDescriptor::appleEventWithEventClass_eventID_targetDescriptor_returnID_transactionID(
        kCoreEventClass,
        kAEOpenApplication,
        Some(&target),
        return_id,
        kAnyTransactionID,
    )
}

/// What a launch's completion reports: the application's process, or why it
/// failed.
///
/// # Safety
///
/// Each pointer is null or points to a live object.
unsafe fn completion_outcome(
    application: *mut NSRunningApplication,
    error: *mut NSError,
) -> Result<libc::pid_t, String> {
    // SAFETY: the caller hands live objects or null.
    let (application, error) = unsafe { (application.as_ref(), error.as_ref()) };
    match (application, error) {
        (_, Some(error)) => Err(describe_error(error)),
        (Some(application), None) => Ok(application.processIdentifier()),
        (None, None) => {
            Err("LaunchServices reported neither an application nor an error".to_owned())
        }
    }
}

fn describe_error(error: &NSError) -> String {
    format!(
        "{} error {}: {}",
        error.domain(),
        error.code(),
        error.localizedDescription()
    )
}

/// The processes of the running instances of `bundle_id`.
fn running_pids(bundle_id: &NSString) -> Vec<libc::pid_t> {
    NSRunningApplication::runningApplicationsWithBundleIdentifier(bundle_id)
        .iter()
        .filter(|application| !application.isTerminated())
        .map(|application| application.processIdentifier())
        .filter(|&pid| pid > 0)
        .collect()
}

/// A running process that stands for the launch: any of `running`, or with
/// `new_instance` one that was not running `before` it.
fn reconciled_pid(
    running: &[libc::pid_t],
    before: &[libc::pid_t],
    new_instance: bool,
) -> Option<libc::pid_t> {
    running
        .iter()
        .copied()
        .find(|pid| !new_instance || !before.contains(pid))
}

/// The launch's completion, or a process `reconcile` finds, whichever comes
/// first within `timeout`; `None` when neither does. A completion already
/// delivered wins over a process, since it carries the error
/// `LaunchServices` reports.
fn launch_signal(
    completion: &mpsc::Receiver<Result<libc::pid_t, String>>,
    timeout: Duration,
    mut reconcile: impl FnMut() -> Option<libc::pid_t>,
) -> Result<Option<Result<libc::pid_t, String>>, Error> {
    let mut signal = None;
    run_loop::spin(timeout, placement::RECHECK, || {
        signal = completion.try_recv().ok().or_else(|| reconcile().map(Ok));
        Ok(signal.is_some())
    })?;
    Ok(signal)
}

#[cfg(test)]
mod tests {
    use objc2_foundation::ns_string;

    use super::*;

    #[test]
    fn a_launch_neither_activates_nor_adds_to_recent_items_and_opens_the_app() {
        for new_instance in [false, true] {
            let configuration = open_configuration("com.apple.TextEdit", new_instance);
            assert!(!configuration.activates());
            assert!(!configuration.addsToRecentItems());
            assert_eq!(configuration.createsNewApplicationInstance(), new_instance);
            let event = configuration
                .appleEvent()
                .expect("the launch carries an Apple event");
            assert_eq!(event.eventClass(), kCoreEventClass);
            assert_eq!(event.eventID(), kAEOpenApplication);
        }
    }

    #[test]
    fn the_config_collects_what_to_open() {
        let config = LaunchConfig::bundle_id("com.apple.TextEdit")
            .file("notes.txt")
            .url("https://example.com/")
            .new_instance(true)
            .display(7)
            .timeout(Duration::from_secs(5));
        assert_eq!(config.app, App::BundleId("com.apple.TextEdit".to_owned()));
        assert_eq!(
            config.open,
            [
                Open::File(PathBuf::from("notes.txt")),
                Open::Url("https://example.com/".to_owned())
            ]
        );
        assert!(config.new_instance);
        assert_eq!(config.display, Some(7));
        assert_eq!(config.timeout, Duration::from_secs(5));
        let defaults = LaunchConfig::bundle("/System/Applications/TextEdit.app");
        assert!(!defaults.new_instance);
        assert_eq!(defaults.display, None);
        assert_eq!(defaults.timeout, DEFAULT_TIMEOUT);
    }

    /// `LaunchServices` lookups only; nothing launches.
    #[test]
    fn an_app_resolves_by_bundle_id_and_by_path() {
        let by_id = ResolvedApp::resolve(&App::BundleId("com.apple.TextEdit".to_owned()))
            .expect("TextEdit is installed");
        assert_eq!(by_id.bundle_id, "com.apple.TextEdit");
        let by_path = ResolvedApp::resolve(&App::Bundle(PathBuf::from(
            "/System/Applications/TextEdit.app",
        )))
        .expect("TextEdit's bundle resolves");
        assert_eq!(by_path.bundle_id, "com.apple.TextEdit");
    }

    /// Reads the installed applications; nothing launches.
    #[test]
    fn an_app_resolves_by_name_to_its_bundle_id() {
        for name in ["Finder", "finder", "FINDER"] {
            let app =
                ResolvedApp::resolve(&App::Name(name.to_owned())).expect("Finder is installed");
            assert_eq!(app.bundle_id, "com.apple.finder", "{name}");
        }
        let calculator = ResolvedApp::resolve(&App::Name("Calculator".to_owned()))
            .expect("Calculator is installed");
        assert_eq!(calculator.bundle_id, "com.apple.calculator");
    }

    #[test]
    fn an_unknown_name_is_refused_with_the_closest_names() {
        let unknown = "zzz-not-an-installed-app";
        assert!(matches!(
            ResolvedApp::resolve(&App::Name(unknown.to_owned())),
            Err(Error::AppNameNotFound { name, suggestions })
                if name == unknown && suggestions.is_empty()
        ));
        // A name found only inside others suggests them without matching.
        let Err(Error::AppNameNotFound { suggestions, .. }) =
            ResolvedApp::resolve(&App::Name("inder".to_owned()))
        else {
            panic!("a name inside another is not a match");
        };
        assert!(suggestions.len() <= crate::app_names::SUGGESTION_LIMIT);
    }

    #[test]
    fn equally_good_names_are_refused_as_ambiguous() {
        let root = tempfile::tempdir().expect("a temporary directory");
        for name in ["Notes One", "Notes Two"] {
            let contents = root.path().join(format!("{name}.app/Contents"));
            std::fs::create_dir_all(&contents).expect("the bundle directory");
            std::fs::write(
                contents.join("Info.plist"),
                format!(
                    r#"<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>test.{}</string></dict></plist>"#,
                    name.replace(' ', "-")
                ),
            )
            .expect("the Info.plist");
        }
        let roots = [root.path().to_path_buf()];
        assert!(matches!(
            named_bundle("notes", &roots, &[]),
            Err(Error::AppNameAmbiguous { name, suggestions })
                if name == "notes" && suggestions == ["Notes One", "Notes Two"]
        ));
        assert_eq!(
            named_bundle("Notes Two", &roots, &[]).expect("an exact name resolves"),
            root.path().join("Notes Two.app")
        );
    }

    #[test]
    fn the_name_form_is_collected_by_the_config() {
        let config = LaunchConfig::name("Google Chrome").url("https://example.com/");
        assert_eq!(config.app, App::Name("Google Chrome".to_owned()));
        assert_eq!(config.open, [Open::Url("https://example.com/".to_owned())]);
    }

    #[test]
    fn an_unknown_or_missing_app_is_refused() {
        let unknown = "dev.waterkit.automation.not-installed";
        assert!(matches!(
            ResolvedApp::resolve(&App::BundleId(unknown.to_owned())),
            Err(Error::AppNotFound(id)) if id == unknown
        ));
        let missing = PathBuf::from("/System/Applications/Not Installed.app");
        assert!(matches!(
            ResolvedApp::resolve(&App::Bundle(missing.clone())),
            Err(Error::FileNotFound(path)) if path == missing
        ));
        let not_a_bundle = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        assert!(matches!(
            ResolvedApp::resolve(&App::Bundle(not_a_bundle.clone())),
            Err(Error::NotAnAppBundle(path)) if path == not_a_bundle
        ));
    }

    #[test]
    fn urls_need_a_scheme() {
        let url = Open::Url("https://example.com/文書/書類/문서".to_owned())
            .resolve()
            .expect("an absolute URL resolves");
        assert_eq!(url.scheme().expect("it has a scheme").to_string(), "https");
        assert!(matches!(
            Open::Url("example.com/page".to_owned()).resolve(),
            Err(Error::InvalidUrl(url)) if url == "example.com/page"
        ));
    }

    #[test]
    fn files_resolve_to_absolute_file_urls() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let url = Open::File(manifest.clone())
            .resolve()
            .expect("the manifest exists");
        assert!(url.isFileURL());
        assert_eq!(
            url.path().expect("a file URL has a path").to_string(),
            manifest.to_str().expect("the manifest path is UTF-8")
        );
        let missing = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("missing.txt");
        assert!(matches!(
            Open::File(missing.clone()).resolve(),
            Err(Error::FileNotFound(path)) if path == missing
        ));
    }

    #[test]
    fn a_failed_launch_reports_the_error() {
        let error = NSError::new(-10827, ns_string!("NSOSStatusErrorDomain"));
        // SAFETY: `error` is live and no application is handed over.
        let outcome = unsafe {
            completion_outcome(core::ptr::null_mut(), Retained::as_ptr(&error).cast_mut())
        };
        let reason = outcome.expect_err("an error fails the launch");
        assert!(
            reason.starts_with("NSOSStatusErrorDomain error -10827: "),
            "{reason}"
        );
        // SAFETY: both are null.
        let outcome = unsafe { completion_outcome(core::ptr::null_mut(), core::ptr::null_mut()) };
        assert!(outcome.is_err());
    }

    #[test]
    fn a_new_instance_is_a_process_that_was_not_running() {
        let before = [100];
        assert_eq!(reconciled_pid(&[100], &before, true), None);
        assert_eq!(reconciled_pid(&[100, 200], &before, true), Some(200));
        assert_eq!(reconciled_pid(&[100], &before, false), Some(100));
        assert_eq!(reconciled_pid(&[], &[], false), None);
    }

    #[test]
    fn a_process_stands_for_a_lost_completion() {
        let (_sender, completion) = mpsc::sync_channel(1);
        let mut checks = 0;
        let signal = launch_signal(&completion, Duration::from_secs(5), || {
            checks += 1;
            (checks == 3).then_some(4242)
        })
        .expect("the wait runs");
        assert_eq!(signal, Some(Ok(4242)));
    }

    #[test]
    fn a_delivered_failure_wins_over_a_process() {
        let (sender, completion) = mpsc::sync_channel(1);
        sender
            .send(Err("denied".to_owned()))
            .expect("the channel has room");
        let signal = launch_signal(&completion, Duration::from_secs(5), || Some(4242))
            .expect("the wait runs");
        assert_eq!(signal, Some(Err("denied".to_owned())));
    }

    #[test]
    fn neither_signal_times_the_wait_out() {
        let (_sender, completion) = mpsc::sync_channel::<Result<libc::pid_t, String>>(1);
        let signal =
            launch_signal(&completion, Duration::from_millis(20), || None).expect("the wait runs");
        assert_eq!(signal, None);
    }
}
