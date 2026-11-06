//! macOS privacy grants that need helper instances of the app: Screen
//! Recording, Accessibility and the direct-capture consent.
//!
//! The parent side never asks TCC itself, because macOS caches a negative
//! answer in the asking process for its lifetime. It reads status from a
//! fresh child of its own executable and requests through an instance
//! `LaunchServices` launches; see [`crate::helper`] for the protocol.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Output};
use std::ptr::NonNull;
use std::time::Duration;

use async_process::{Command, Stdio};
use futures::FutureExt as _;
use objc2_application_services::{
    AXIsProcessTrusted, AXIsProcessTrustedWithOptions, kAXTrustedCheckOptionPrompt,
};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFURL};
use objc2_core_graphics::{
    CGMainDisplayID, CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess,
};
use objc2_foundation::NSBundle;
use objc2_security::{
    SecCSFlags, SecCode, SecCodeSignatureFlags, SecStaticCode, kSecCSSigningInformation,
    kSecCodeInfoFlags,
};

use crate::helper::{Grant, GrantReport, HelperCommand};
use crate::{Permission, PermissionError, PermissionStatus};

/// How long a status helper may take to print its report. It does two TCC
/// queries and exits, so running out means the app's `main` did not hand
/// the launch to the helper entry.
const STATUS_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a request instance may take to exit. It raises the system
/// prompt, which answers asynchronously, and exits; the direct-capture
/// request also captures the main display once.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

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
        HelperCommand::Request(Grant::ScreenRecording) => {
            // The prompt answers asynchronously; the return value is the
            // grant as it stood before the user saw it.
            CGRequestScreenCaptureAccess();
        }
        HelperCommand::Request(Grant::Accessibility) => {
            // SAFETY: an immutable string constant ApplicationServices
            // exports.
            let key: &CFString = unsafe { kAXTrustedCheckOptionPrompt };
            let options = CFDictionary::from_slices(&[key], &[CFBoolean::new(true)]);
            // SAFETY: the options dictionary maps the documented key to a
            // CFBoolean, as AXIsProcessTrustedWithOptions expects. Prompting
            // is asynchronous and does not change the return value.
            unsafe { AXIsProcessTrustedWithOptions(Some(options.as_opaque())) };
        }
        HelperCommand::Request(Grant::DirectScreenCapture) => {
            // replayd asks for the consent at the app's first capture that
            // bypasses the picker. Only that prompt matters: the image the
            // capture produces depends on the answer the user is still
            // giving, so it is dropped unread.
            #[expect(
                deprecated,
                reason = "a whole-display still with no stream is exactly the capture that raises the consent"
            )]
            drop(objc2_core_graphics::CGDisplayCreateImage(CGMainDisplayID()));
        }
    }
    ExitCode::SUCCESS
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

/// `open -n -W -g <app> --args <request command>`: a new instance (`-n`)
/// launched by `LaunchServices`, kept in the background (`-g`) so only the
/// system prompt takes focus, and waited for (`-W`).
async fn launch_request_instance(app: &Path, grant: Grant) -> Result<(), String> {
    let output = bounded(
        Command::new("/usr/bin/open")
            .args(["-n", "-W", "-g"])
            .arg(app)
            .arg("--args")
            .args(HelperCommand::Request(grant).args())
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
        REQUEST_TIMEOUT,
    )
    .await
    .ok_or_else(|| {
        format!(
            "the {} request instance of {} did not exit within {REQUEST_TIMEOUT:?}",
            grant.arg(),
            app.display()
        )
    })?
    .map_err(|error| format!("launch the {} request instance: {error}", grant.arg()))?;
    check_exit(&format!("the {} request instance", grant.arg()), &output)
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
    use objc2_security::SecCodeSignatureFlags;

    use super::{ENTRY_HINT, fresh_report, signature_flags};

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
