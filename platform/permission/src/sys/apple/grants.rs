//! macOS privacy grants that need helper instances of the app: Screen
//! Recording, Accessibility and the direct-capture consent.
//!
//! The parent side never asks TCC itself, because macOS caches a negative
//! answer in the asking process for its lifetime. It reads status from a
//! fresh child of its own executable and requests through an instance
//! `LaunchServices` launches; see [`crate::helper`] for the protocol.

use core::cell::RefCell;
use std::io::Write as _;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Output};
use std::ptr::NonNull;
use std::time::Duration;

use async_io::Async;
use async_process::{Command, Stdio};
use block2::RcBlock;
use futures::channel::oneshot;
use futures::{AsyncReadExt as _, FutureExt as _};
use objc2::rc::Retained;
use objc2::{AnyThread as _, available};
use objc2_application_services::{
    AXIsProcessTrusted, AXIsProcessTrustedWithOptions, kAXTrustedCheckOptionPrompt,
};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFURL};
use objc2_core_graphics::{
    CGImage, CGMainDisplayID, CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess,
};
use objc2_foundation::{NSArray, NSBundle, NSError};
use objc2_screen_capture_kit::{
    SCContentFilter, SCScreenshotManager, SCShareableContent, SCStreamConfiguration, SCWindow,
};
use objc2_security::{
    SecCSFlags, SecCode, SecCodeSignatureFlags, SecStaticCode, kSecCSSigningInformation,
    kSecCodeInfoFlags,
};

use crate::helper::{Grant, GrantReport, HelperCommand, RequestReport};
use crate::{Permission, PermissionError, PermissionStatus};

/// How long a status helper may take to print its report. It does two TCC
/// queries and exits, so running out means the app's `main` did not hand
/// the launch to the helper entry.
const STATUS_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the direct-capture request waits for its capture to complete;
/// the capture may wait on the user answering replayd's consent alert.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(120);

/// How long a request instance may take to launch, report and exit. It
/// raises the system prompt, which answers asynchronously, and exits; the
/// direct-capture request first waits up to [`CAPTURE_TIMEOUT`] for its
/// capture.
const REQUEST_TIMEOUT: Duration = CAPTURE_TIMEOUT.saturating_add(Duration::from_secs(10));

// MARK: - Helper side

/// Runs `command` in this helper instance.
pub fn run(command: HelperCommand) -> ExitCode {
    match command {
        HelperCommand::Status => {
            let report = GrantReport {
                screen_recording: CGPreflightScreenCaptureAccess(),
                // SAFETY: a TCC query with no arguments.
                accessibility: unsafe { AXIsProcessTrusted() },
            };
            std::io::stdout()
                .write_all(&report.encode())
                .expect("the parent reads the grant report from stdout");
        }
        HelperCommand::Request { grant, report } => {
            let reporter = Reporter::connect(&report);
            reporter.report_panics();
            raise_prompt(grant);
            reporter.send(&RequestReport::Completed);
        }
    }
    ExitCode::SUCCESS
}

/// Raises `grant`'s system prompt.
fn raise_prompt(grant: Grant) {
    match grant {
        Grant::ScreenRecording => {
            // The prompt answers asynchronously; the return value is the
            // grant as it stood before the user saw it.
            CGRequestScreenCaptureAccess();
        }
        Grant::Accessibility => {
            // SAFETY: an immutable string constant ApplicationServices
            // exports.
            let key: &CFString = unsafe { kAXTrustedCheckOptionPrompt };
            let options = CFDictionary::from_slices(&[key], &[CFBoolean::new(true)]);
            // SAFETY: the options dictionary maps the documented key to a
            // CFBoolean, as AXIsProcessTrustedWithOptions expects. Prompting
            // is asynchronous and does not change the return value.
            unsafe { AXIsProcessTrustedWithOptions(Some(options.as_opaque())) };
        }
        Grant::DirectScreenCapture => raise_direct_capture_consent(),
    }
}

/// The request instance's end of the socket the app listens on. The
/// instance holds it until it exits, so the app reads to its end.
struct Reporter(UnixStream);

impl Reporter {
    /// Connects to the app's socket at `path`.
    ///
    /// # Panics
    ///
    /// Panics when the socket does not accept the connection; the app then
    /// fails its request when no report arrives in time.
    fn connect(path: &Path) -> Self {
        Self(UnixStream::connect(path).unwrap_or_else(|error| {
            panic!(
                "connect to the request report socket {}: {error}",
                path.display()
            )
        }))
    }

    /// Sends a panic's description before the panic ends this instance,
    /// under `panic = "abort"` as much as under unwinding.
    fn report_panics(&self) {
        let stream = self
            .0
            .try_clone()
            .expect("duplicate the request report socket for the panic hook");
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let report = RequestReport::Panicked(info.to_string());
            // A write the app is no longer reading has no one to tell.
            let _ = (&stream).write_all(&report.encode());
            previous(info);
        }));
    }

    /// Sends `report`.
    ///
    /// # Panics
    ///
    /// Panics when the app stopped reading; the panic report then fails
    /// too, and the app sees the socket close without a report.
    fn send(&self, report: &RequestReport) {
        (&self.0)
            .write_all(&report.encode())
            .expect("send the request report to the app");
    }
}

/// Captures the main display once through `SCScreenshotManager` and waits
/// for the capture to complete.
///
/// replayd asks for the consent at the app's first capture that bypasses
/// the picker. The capture is asynchronous, and one abandoned by an exiting
/// process dismisses the consent alert, so this returns only once the
/// completion handler ran. Whatever it delivers, an image or an error,
/// answers the consent the user gave and is dropped unread.
///
/// # Panics
///
/// Panics before macOS 14, which has no `SCScreenshotManager`, when
/// `ScreenCaptureKit` lists no main display or drops the completion handler,
/// and when the capture does not complete within [`CAPTURE_TIMEOUT`].
fn raise_direct_capture_consent() {
    assert!(
        available!(macos = 14.0, ..),
        "the direct-capture consent request captures through SCScreenshotManager, which needs \
         macOS 14"
    );
    let (sender, receiver) = oneshot::channel::<Result<(), String>>();
    let pending = RefCell::new(Some(sender));
    let content_handler = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            let Some(sender) = pending.borrow_mut().take() else {
                return;
            };
            // SAFETY: both pointers come from ScreenCaptureKit and stay
            // valid for the duration of the callback.
            let (filter, config) = match unsafe { main_display_capture(content, error) } {
                Ok(capture) => capture,
                Err(error) => {
                    // The receiver lives until the helper gives up waiting.
                    let _ = sender.send(Err(error));
                    return;
                }
            };
            let pending = RefCell::new(Some(sender));
            let captured = RcBlock::new(move |_image: *mut CGImage, _error: *mut NSError| {
                if let Some(sender) = pending.borrow_mut().take() {
                    let _ = sender.send(Ok(()));
                }
            });
            // SAFETY: `filter` and `config` are live objects, and the
            // framework retains the completion block until it runs.
            unsafe {
                SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
                    &filter,
                    &config,
                    Some(&captured),
                );
            }
        },
    );
    // SAFETY: the framework retains the completion block until it runs.
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&content_handler) };
    futures::executor::block_on(bounded(receiver, CAPTURE_TIMEOUT))
        .unwrap_or_else(|| {
            panic!(
                "the direct-capture request's capture did not complete within {CAPTURE_TIMEOUT:?}"
            )
        })
        .expect("ScreenCaptureKit dropped the capture's completion handler")
        .unwrap_or_else(|error| panic!("{error}"));
}

/// The filter and configuration that capture the whole main display.
///
/// # Safety
///
/// `content` and `error` are the arguments of a `SCShareableContent`
/// completion handler, valid for the duration of that callback.
unsafe fn main_display_capture(
    content: *mut SCShareableContent,
    error: *mut NSError,
) -> Result<(Retained<SCContentFilter>, Retained<SCStreamConfiguration>), String> {
    // SAFETY: a null or live NSError, per the caller's contract.
    if let Some(error) = unsafe { error.as_ref() } {
        return Err(format!(
            "list the displays to capture: {}",
            error.localizedDescription()
        ));
    }
    // SAFETY: a null or live SCShareableContent, per the caller's contract.
    let content = unsafe { content.as_ref() }
        .ok_or("ScreenCaptureKit listed no shareable content and no error")?;
    let main = CGMainDisplayID();
    // SAFETY: read-only accessors on live ScreenCaptureKit objects.
    let display = unsafe { content.displays() }
        .iter()
        .find(|display| unsafe { display.displayID() } == main)
        .ok_or_else(|| format!("ScreenCaptureKit lists no main display (id {main})"))?;
    let no_windows = NSArray::<SCWindow>::new();
    // SAFETY: `display` and the empty exclusion list are live objects; the
    // configuration is a plain data object whose defaults suffice for an
    // image nobody reads.
    let capture = unsafe {
        (
            SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                &display,
                &no_windows,
            ),
            SCStreamConfiguration::new(),
        )
    };
    Ok(capture)
}

// MARK: - App side

/// The status of `grant`, read in a fresh helper.
///
/// # Panics
///
/// Panics when the status helper fails: [`crate::check`] has no error to
/// report it through, and every such failure is a bug or a broken install.
pub async fn check(grant: Grant) -> PermissionStatus {
    fresh_report()
        .await
        .unwrap_or_else(|error| panic!("{error}"))
        .status(grant)
}

/// Raises `grant`'s system prompt from a `LaunchServices`-launched instance
/// unless it is already granted, then reads the status again.
///
/// The prompt answers asynchronously, so the status returned is the one in
/// force when the request instance exits; the user may still be deciding.
pub async fn request(grant: Grant) -> Result<PermissionStatus, PermissionError> {
    let before = fresh_report().await.map_err(PermissionError::Platform)?;
    match (grant, before.status(grant)) {
        (_, PermissionStatus::Granted) => return Ok(PermissionStatus::Granted),
        (Grant::DirectScreenCapture, PermissionStatus::NotGranted) => {
            return Err(PermissionError::Prerequisite(Permission::ScreenRecording));
        }
        _ => {}
    }
    let app = signed_app_bundle().map_err(PermissionError::Platform)?;
    launch_request_instance(&app, grant)
        .await
        .map_err(PermissionError::Platform)?;
    Ok(fresh_report()
        .await
        .map_err(PermissionError::Platform)?
        .status(grant))
}

/// Runs a status helper and decodes its report.
async fn fresh_report() -> Result<GrantReport, String> {
    let exe = std::env::current_exe().map_err(|error| {
        format!("locate the app executable for the grant status helper: {error}")
    })?;
    let output = bounded(
        Command::new(&exe)
            .args(HelperCommand::Status.args())
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
        STATUS_TIMEOUT,
    )
    .await
    .ok_or_else(|| {
        format!(
            "the grant status helper ({}) printed no report within {STATUS_TIMEOUT:?}; {ENTRY_HINT}",
            exe.display()
        )
    })?
    .map_err(|error| format!("run the grant status helper ({}): {error}", exe.display()))?;
    check_exit(
        &format!("the grant status helper ({})", exe.display()),
        &output,
    )
    .map_err(|error| format!("{error}; {ENTRY_HINT}"))?;
    GrantReport::decode(&output.stdout).map_err(|error| {
        format!(
            "the grant status helper ({}) printed no grant report ({error}); {ENTRY_HINT}",
            exe.display()
        )
    })
}

/// What an app whose status helper misbehaves most likely lacks: a launch
/// its `main` does not hand to the entry runs the app itself instead.
const ENTRY_HINT: &str =
    "the app's main must call waterkit_permission::run_helper_entry_if_requested first";

/// `open -n -g <app> --args <request command>`: a new instance (`-n`)
/// launched by `LaunchServices`, kept in the background (`-g`) so only the
/// system prompt takes focus; it reports through a [`ReportSocket`] and
/// returns once the instance exited.
///
/// `open -W` cannot wait for it: `open` watches the pid only after the
/// launch completes, and an instance that has already exited by then makes
/// it fail (`kevent` reports ESRCH) or return success unwatched, while the
/// instance's own exit status never reaches `open`'s.
async fn launch_request_instance(app: &Path, grant: Grant) -> Result<(), String> {
    let socket = ReportSocket::bind()?;
    let launch = Command::new("/usr/bin/open")
        .args(["-n", "-g"])
        .arg(app)
        .arg("--args")
        .args(
            HelperCommand::Request {
                grant,
                report: socket.path.clone(),
            }
            .args(),
        )
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output();
    request_outcome(
        &format!("the {} request instance of {}", grant.arg(), app.display()),
        launch,
        socket,
        REQUEST_TIMEOUT,
    )
    .await
}

/// Waits up to `timeout` for `launch` to start the request instance and
/// for the instance's report through `socket`, ending when the instance
/// exited. `what` names the instance in errors.
async fn request_outcome(
    what: &str,
    launch: impl Future<Output = std::io::Result<Output>>,
    socket: ReportSocket,
    timeout: Duration,
) -> Result<(), String> {
    let outcome = async {
        let output = launch
            .await
            .map_err(|error| format!("launch {what}: {error}"))?;
        check_exit(&format!("/usr/bin/open launching {what}"), &output)?;
        let report = socket.read_to_close(what).await?;
        if report.is_empty() {
            return Err(format!(
                "{what} exited without reporting how its request ended; it crashed or was killed"
            ));
        }
        match RequestReport::decode(&report) {
            Ok(RequestReport::Completed) => Ok(()),
            Ok(RequestReport::Panicked(panic)) => Err(format!("{what} {panic}")),
            Err(error) => Err(format!(
                "{what} sent no single request report ({error}): {}",
                String::from_utf8_lossy(&report)
            )),
        }
    };
    bounded(outcome, timeout)
        .await
        .unwrap_or_else(|| Err(format!("{what} did not report and exit within {timeout:?}")))
}

/// A socket in a directory of its own that a request instance connects to
/// and reports through. Dropping it removes both.
struct ReportSocket {
    listener: Async<UnixListener>,
    path: PathBuf,
    _dir: tempfile::TempDir,
}

impl ReportSocket {
    fn bind() -> Result<Self, String> {
        let dir = tempfile::Builder::new()
            .prefix("waterkit-")
            .tempdir()
            .map_err(|error| {
                format!("create the directory for the request report socket: {error}")
            })?;
        let path = dir.path().join("report");
        let listener = Async::<UnixListener>::bind(&path).map_err(|error| {
            format!("bind the request report socket {}: {error}", path.display())
        })?;
        Ok(Self {
            listener,
            path,
            _dir: dir,
        })
    }

    /// Everything the instance sends on its one connection, up to the close
    /// its exit causes. `what` names the instance in errors.
    async fn read_to_close(&self, what: &str) -> Result<Vec<u8>, String> {
        let (mut stream, _) = self
            .listener
            .accept()
            .await
            .map_err(|error| format!("accept the report connection of {what}: {error}"))?;
        let mut report = Vec::new();
        stream
            .read_to_end(&mut report)
            .await
            .map_err(|error| format!("read the report of {what}: {error}"))?;
        Ok(report)
    }
}

/// `future`'s output, or `None` once `timeout` passes first.
async fn bounded<T>(future: impl Future<Output = T>, timeout: Duration) -> Option<T> {
    futures::select_biased! {
        output = future.fuse() => Some(output),
        _ = async_io::Timer::after(timeout).fuse() => None,
    }
}

/// An error naming `what` and its stderr unless `output` reports success.
fn check_exit(what: &str, output: &Output) -> Result<(), String> {
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "{what} exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

/// The `.app` bundle this process runs from, signed with a certificate.
///
/// TCC identifies the app by its designated requirement. An ad hoc
/// signature (the linker's included) makes that a bare cdhash, so every
/// rebuild loses the grants, and macOS 27 never credits Accessibility to
/// such a bundle at all.
fn signed_app_bundle() -> Result<PathBuf, String> {
    let bundle = NSBundle::mainBundle();
    let path = PathBuf::from(bundle.bundlePath().to_string());
    if bundle.bundleIdentifier().is_none()
        || path.extension().is_none_or(|extension| extension != "app")
    {
        return Err(format!(
            "requesting macOS privacy grants needs the app to run from its .app bundle, and {} \
             is not one",
            path.display()
        ));
    }
    if signature_flags(&path)?.contains(SecCodeSignatureFlags::Adhoc) {
        return Err(format!(
            "{} carries an ad hoc signature; macOS ties privacy grants to a bundle signed with a \
             certificate",
            path.display()
        ));
    }
    Ok(path)
}

/// The code-signature flags of the bundle or executable at `path`.
fn signature_flags(path: &Path) -> Result<SecCodeSignatureFlags, String> {
    let url = CFURL::from_file_path(path)
        .ok_or_else(|| format!("{} is not a file URL", path.display()))?;
    let mut code: *const SecStaticCode = std::ptr::null();
    // SAFETY: `code` is a valid out-pointer; on success it holds a +1
    // reference that `from_raw` takes over.
    let status = unsafe {
        SecStaticCode::create_with_path(&url, SecCSFlags::DefaultFlags, NonNull::from(&mut code))
    };
    let code = NonNull::new(code.cast_mut())
        .filter(|_| status == 0)
        // SAFETY: a +1 reference SecStaticCodeCreateWithPath returned.
        .map(|code| unsafe { CFRetained::from_raw(code) })
        .ok_or_else(|| {
            format!(
                "read the code signature of {} (OSStatus {status})",
                path.display()
            )
        })?;
    let mut info: *const CFDictionary = std::ptr::null();
    // SAFETY: `info` is a valid out-pointer; on success it holds a +1
    // reference that `from_raw` takes over.
    let status = unsafe {
        SecCode::copy_signing_information(
            &code,
            SecCSFlags(kSecCSSigningInformation),
            NonNull::from(&mut info),
        )
    };
    let info = NonNull::new(info.cast_mut())
        .filter(|_| status == 0)
        // SAFETY: a +1 reference SecCodeCopySigningInformation returned.
        .map(|info| unsafe { CFRetained::from_raw(info) })
        .ok_or_else(|| {
            format!(
                "read the signing information of {} (OSStatus {status})",
                path.display()
            )
        })?;
    // SAFETY: the signing-information dictionary maps CFString keys to
    // property-list values, and kSecCodeInfoFlags is a CFNumber.
    let info = unsafe { info.cast_unchecked::<CFString, CFNumber>() };
    // SAFETY: an immutable string constant Security exports.
    let key: &CFString = unsafe { kSecCodeInfoFlags };
    let flags = info
        .get(key)
        .and_then(|flags| flags.as_i64())
        .ok_or_else(|| format!("{} carries no code signature", path.display()))?;
    let flags = u32::try_from(flags)
        .unwrap_or_else(|_| panic!("code-signature flags {flags:#x} do not fit in 32 bits"));
    Ok(SecCodeSignatureFlags(flags))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use async_process::Command;
    use objc2_security::SecCodeSignatureFlags;

    use super::{
        ENTRY_HINT, ReportSocket, Reporter, RequestReport, fresh_report, request_outcome,
        signature_flags,
    };

    const INSTANCE: &str = "the screen-recording request instance";

    /// Far more than a report already waiting in the socket takes to read.
    const TIMEOUT: Duration = Duration::from_secs(2);

    /// The outcome of a launch that succeeds and an instance that does what
    /// `instance` does with its end of the socket and exits, all before the
    /// app starts waiting: no instance exits too fast for the app.
    fn outcome_of(instance: impl FnOnce(Reporter)) -> Result<(), String> {
        let socket = ReportSocket::bind().unwrap();
        instance(Reporter::connect(&socket.path));
        futures::executor::block_on(request_outcome(
            INSTANCE,
            Command::new("/usr/bin/true").output(),
            socket,
            TIMEOUT,
        ))
    }

    #[test]
    fn a_completed_request_succeeds() {
        outcome_of(|reporter| reporter.send(&RequestReport::Completed)).unwrap();
    }

    #[test]
    fn a_panicked_request_fails_with_the_panic() {
        let error = outcome_of(|reporter| {
            reporter.send(&RequestReport::Panicked(
                "panicked at src/main.rs:3:5:\n画面収録 화면 녹화 屏幕录制".into(),
            ));
        })
        .unwrap_err();
        assert_eq!(
            error,
            format!("{INSTANCE} panicked at src/main.rs:3:5:\n画面収録 화면 녹화 屏幕录制")
        );
    }

    #[test]
    fn an_instance_that_exits_without_a_report_fails() {
        let error = outcome_of(drop).unwrap_err();
        assert!(
            error.starts_with(&format!(
                "{INSTANCE} exited without reporting how its request ended"
            )),
            "{error}"
        );
    }

    #[test]
    fn a_report_followed_by_more_fails() {
        let error = outcome_of(|reporter| {
            reporter.send(&RequestReport::Completed);
            reporter.send(&RequestReport::Panicked("late".into()));
        })
        .unwrap_err();
        assert!(
            error.starts_with(&format!("{INSTANCE} sent no single request report")),
            "{error}"
        );
    }

    #[test]
    fn an_instance_that_never_reports_times_out() {
        let socket = ReportSocket::bind().unwrap();
        let error = futures::executor::block_on(request_outcome(
            INSTANCE,
            Command::new("/usr/bin/true").output(),
            socket,
            Duration::from_millis(200),
        ))
        .unwrap_err();
        assert_eq!(
            error,
            format!("{INSTANCE} did not report and exit within 200ms")
        );
    }

    /// An instance still running after its report holds the app until it
    /// exits, and then the bound ends the wait.
    #[test]
    fn an_instance_that_does_not_exit_times_out() {
        let socket = ReportSocket::bind().unwrap();
        let path = socket.path.clone();
        let (exit, exited) = std::sync::mpsc::channel::<()>();
        let instance = std::thread::spawn(move || {
            let reporter = Reporter::connect(&path);
            reporter.send(&RequestReport::Completed);
            exited.recv().unwrap_err();
        });
        let error = futures::executor::block_on(request_outcome(
            INSTANCE,
            Command::new("/usr/bin/true").output(),
            socket,
            Duration::from_millis(200),
        ))
        .unwrap_err();
        drop(exit);
        instance.join().unwrap();
        assert_eq!(
            error,
            format!("{INSTANCE} did not report and exit within 200ms")
        );
    }

    #[test]
    fn a_failed_launch_fails_with_its_error() {
        let socket = ReportSocket::bind().unwrap();
        let error = futures::executor::block_on(request_outcome(
            INSTANCE,
            Command::new("/bin/sh")
                .args([
                    "-c",
                    "echo '_LSOpenURLsWithCompletionHandler() failed with error -10810.' >&2; \
                     exit 1",
                ])
                .output(),
            socket,
            TIMEOUT,
        ))
        .unwrap_err();
        assert_eq!(
            error,
            format!(
                "/usr/bin/open launching {INSTANCE} exited with exit status: 1: \
                 _LSOpenURLsWithCompletionHandler() failed with error -10810."
            )
        );
    }

    #[test]
    fn the_socket_and_its_directory_go_with_it() {
        let socket = ReportSocket::bind().unwrap();
        let dir = socket.path.parent().unwrap().to_owned();
        assert!(socket.path.exists());
        drop(socket);
        assert!(!dir.exists(), "{}", dir.display());
    }

    /// The libtest harness is an app whose `main` does not wire the entry:
    /// it rejects the helper flag, and the read names what is missing.
    #[test]
    fn an_unwired_app_fails_naming_the_entry() {
        let error = futures::executor::block_on(fresh_report()).unwrap_err();
        assert!(error.ends_with(ENTRY_HINT), "{error}");
    }

    /// Cargo's test executables carry the linker's ad hoc signature.
    #[test]
    fn a_linker_signed_executable_reads_ad_hoc() {
        let exe = std::env::current_exe().unwrap();
        let flags = signature_flags(&exe).unwrap();
        assert!(flags.contains(SecCodeSignatureFlags::Adhoc), "{flags:?}");
        assert!(
            flags.contains(SecCodeSignatureFlags::LinkerSigned),
            "{flags:?}"
        );
    }
}
