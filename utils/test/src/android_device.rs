//! Brings the Android device into the state the harness needs before it
//! launches: awake, past the keyguard, and with the harness window focused.
//!
//! The harness activity starts its tests only once its window has focus, and a
//! window never gets focus while the device is asleep, dozing, or behind the
//! keyguard. Each of those states is read from the system services that own
//! it, so a device that cannot reach the required state fails with the reason
//! instead of timing out later as a missing test report.

use crate::{ANDROID_HARNESS_PACKAGE, AndroidToolchain, run_adb, run_adb_with_timeout};
use eyre::{Context, Result, eyre};
use std::str::FromStr;
use std::thread;
use std::time::{Duration, Instant};
use tracing::info;

/// How long the device has to report itself awake after `KEYCODE_WAKEUP`, and
/// to drop a dismissible keyguard after `wm dismiss-keyguard`.
///
/// A Pixel 9 Pro reports `Awake` from `Dozing` within one poll; the bound only
/// keeps a device that ignores the request from stalling the run.
const WAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the harness window has to receive focus once Android reports its
/// first frame.
const FOCUS_TIMEOUT: Duration = Duration::from_secs(10);

/// Upper bound for a single `dumpsys` query over `adb`.
const QUERY_TIMEOUT: Duration = Duration::from_secs(10);

/// Cadence for re-reading device state while waiting for it to change.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Wakes the device and dismisses its keyguard, waiting until the power
/// manager reports it awake and the window manager reports no keyguard.
///
/// Fails at once when the keyguard is secured by a credential: only the person
/// holding the device can unlock it.
pub fn wake_and_unlock(toolchain: &AndroidToolchain) -> Result<()> {
    run_adb(toolchain, ["shell", "input", "keyevent", "KEYCODE_WAKEUP"])
        .context("Failed to send KEYCODE_WAKEUP to the Android device")?;
    let wakefulness = poll_until(
        WAKE_TIMEOUT,
        || read_wakefulness(toolchain),
        |wakefulness| *wakefulness == Wakefulness::Awake,
    )?
    .reached()
    .map_err(|wakefulness| {
        eyre!(
            "The Android device did not wake up: `dumpsys power` still reports \
             mWakefulness={wakefulness:?} {WAKE_TIMEOUT:?} after KEYCODE_WAKEUP. \
             The harness cannot receive window focus while the device is not awake."
        )
    })?;
    info!("Android device is {wakefulness:?}");

    let keyguard = read_keyguard(toolchain)?;
    if !keyguard.showing {
        return Ok(());
    }
    if keyguard.requires_credential() {
        eyre::bail!(
            "The Android device is locked behind a secure lock screen. Unlock it and run \
             again: the harness cannot run its tests behind the keyguard, and only the \
             device's owner can enter its PIN, pattern or password."
        );
    }

    run_adb(toolchain, ["shell", "wm", "dismiss-keyguard"])
        .context("Failed to dismiss the Android keyguard")?;
    poll_until(
        WAKE_TIMEOUT,
        || read_keyguard(toolchain),
        |keyguard| !keyguard.showing,
    )?
    .reached()
    .map_err(|_| {
        eyre!(
            "The Android keyguard is still showing {WAKE_TIMEOUT:?} after `wm dismiss-keyguard`, \
             although it does not require a credential. Unlock the device and run again."
        )
    })?;
    info!("Android keyguard dismissed");
    Ok(())
}

/// Waits until the window manager reports a harness window as the focused
/// window, which is the moment the harness starts its tests.
pub fn wait_for_harness_focus(toolchain: &AndroidToolchain) -> Result<()> {
    poll_until(
        FOCUS_TIMEOUT,
        || read_focused_window(toolchain),
        |focus| focus.belongs_to(ANDROID_HARNESS_PACKAGE),
    )?
    .reached()
    .map_err(|focus| {
        eyre!(
            "The Android test activity was displayed but its window did not receive focus \
             within {FOCUS_TIMEOUT:?} (focused window: {focus}). The harness starts its tests \
             only once its window has focus."
        )
    })?;
    Ok(())
}

/// The outcome of [`poll_until`]: the last state read, and whether it was the
/// one waited for.
enum Polled<T> {
    Reached(T),
    TimedOut(T),
}

impl<T> Polled<T> {
    fn reached(self) -> std::result::Result<T, T> {
        match self {
            Self::Reached(state) => Ok(state),
            Self::TimedOut(state) => Err(state),
        }
    }
}

/// Re-reads device state with `probe` until `done` accepts it or `timeout`
/// elapses.
fn poll_until<T>(
    timeout: Duration,
    mut probe: impl FnMut() -> Result<T>,
    done: impl Fn(&T) -> bool,
) -> Result<Polled<T>> {
    let deadline = Instant::now() + timeout;
    loop {
        let state = probe()?;
        if done(&state) {
            return Ok(Polled::Reached(state));
        }
        if Instant::now() >= deadline {
            return Ok(Polled::TimedOut(state));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn dumpsys(toolchain: &AndroidToolchain, args: &[&str]) -> Result<String> {
    let mut command = vec!["shell", "dumpsys"];
    command.extend_from_slice(args);
    let description = format!("read `dumpsys {}`", args.join(" "));
    let output = run_adb_with_timeout(toolchain, &command, QUERY_TIMEOUT, &description)?;
    if !output.status.success() {
        eyre::bail!(
            "adb could not {description}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout)
        .with_context(|| format!("`dumpsys {}` was not UTF-8", args.join(" ")))
}

fn read_wakefulness(toolchain: &AndroidToolchain) -> Result<Wakefulness> {
    parse_wakefulness(&dumpsys(toolchain, &["power"])?)
}

fn read_keyguard(toolchain: &AndroidToolchain) -> Result<Keyguard> {
    parse_keyguard(&dumpsys(toolchain, &["window", "policy"])?)
}

fn read_focused_window(toolchain: &AndroidToolchain) -> Result<FocusedWindow> {
    parse_focused_window(&dumpsys(toolchain, &["window", "displays"])?)
}

/// The power manager's wakefulness of the default display group, as
/// `PowerManagerInternal.wakefulnessToString` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wakefulness {
    Asleep,
    Awake,
    Dreaming,
    Dozing,
}

impl FromStr for Wakefulness {
    type Err = eyre::Report;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "Asleep" => Ok(Self::Asleep),
            "Awake" => Ok(Self::Awake),
            "Dreaming" => Ok(Self::Dreaming),
            "Dozing" => Ok(Self::Dozing),
            other => Err(eyre!("unknown Android wakefulness `{other}`")),
        }
    }
}

/// Reads `mWakefulness` from the `Power Manager State` section, the first
/// place `dumpsys power` prints it. Later sections print per-group
/// wakefulness as a bare integer, which is not the device-wide state.
fn parse_wakefulness(dumpsys_power: &str) -> Result<Wakefulness> {
    dumpsys_value(dumpsys_power, "mWakefulness")
        .ok_or_else(|| eyre!("`dumpsys power` did not report mWakefulness"))?
        .parse()
        .context("`dumpsys power` reported an unrecognised mWakefulness")
}

/// The keyguard state the window manager's policy holds for the current user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Keyguard {
    showing: bool,
    secure: bool,
    trusted: bool,
}

impl Keyguard {
    /// A secure keyguard needs the user's credential unless a trust agent
    /// currently vouches for the user.
    const fn requires_credential(self) -> bool {
        self.secure && !self.trusted
    }
}

/// Reads the `KeyguardServiceDelegate` block of `dumpsys window policy`.
fn parse_keyguard(dumpsys_window_policy: &str) -> Result<Keyguard> {
    let (_, delegate) = dumpsys_window_policy
        .split_once("KeyguardServiceDelegate")
        .ok_or_else(|| eyre!("`dumpsys window policy` has no KeyguardServiceDelegate section"))?;
    let flag = |key: &str| -> Result<bool> {
        dumpsys_value(delegate, key)
            .ok_or_else(|| eyre!("`dumpsys window policy` KeyguardServiceDelegate has no `{key}`"))?
            .parse()
            .with_context(|| format!("KeyguardServiceDelegate `{key}` is not a boolean"))
    };
    Ok(Keyguard {
        showing: flag("showing")?,
        secure: flag("secure")?,
        trusted: flag("mTrusted")?,
    })
}

/// The window the window manager routes key input to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FocusedWindow {
    None,
    Window(String),
}

impl FocusedWindow {
    /// Whether the focused window is owned by `package`: its activity window
    /// (`package/activity`) or an untitled window it added, such as a dialog,
    /// which the window manager names after the package.
    fn belongs_to(&self, package: &str) -> bool {
        match self {
            Self::None => false,
            Self::Window(tag) => tag
                .strip_prefix(package)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/')),
        }
    }
}

impl std::fmt::Display for FocusedWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::Window(tag) => f.write_str(tag),
        }
    }
}

/// Reads the default display's `mCurrentFocus` from `dumpsys window displays`,
/// printed as `null` or `Window{<id> u<user> <tag>}`.
fn parse_focused_window(dumpsys_window_displays: &str) -> Result<FocusedWindow> {
    let focus = dumpsys_value(dumpsys_window_displays, "mCurrentFocus")
        .ok_or_else(|| eyre!("`dumpsys window displays` did not report mCurrentFocus"))?;
    if focus == "null" {
        return Ok(FocusedWindow::None);
    }
    let tag = focus
        .strip_prefix("Window{")
        .and_then(|window| window.strip_suffix('}'))
        .and_then(|window| window.splitn(3, ' ').nth(2))
        .ok_or_else(|| {
            eyre!("unrecognised mCurrentFocus `{focus}` in `dumpsys window displays`")
        })?;
    Ok(FocusedWindow::Window(tag.to_owned()))
}

/// Finds the first `key=value` line in a `dumpsys` dump.
fn dumpsys_value<'a>(dump: &'a str, key: &str) -> Option<&'a str> {
    dump.lines().find_map(|line| {
        line.trim()
            .strip_prefix(key)?
            .strip_prefix('=')
            .map(str::trim)
    })
}

#[cfg(test)]
mod tests {
    use super::{
        FocusedWindow, Keyguard, Wakefulness, parse_focused_window, parse_keyguard,
        parse_wakefulness,
    };
    use crate::ANDROID_HARNESS_PACKAGE;

    /// `dumpsys power` from a dozing Pixel 9 Pro (Android 16), cut to the
    /// state section and the per-group section that repeats `mWakefulness`.
    const POWER_DOZING: &str = include_str!("../tests/fixtures/dumpsys-power-dozing.txt");

    /// `dumpsys window policy` from a Pixel 9 Pro with no lock screen, dozing.
    const POLICY_NO_LOCK: &str = include_str!("../tests/fixtures/dumpsys-window-policy.txt");

    /// The focus lines of `dumpsys window displays` from a Pixel 9 Pro.
    const DISPLAYS_HARNESS: &str =
        include_str!("../tests/fixtures/dumpsys-window-displays-harness.txt");
    const DISPLAYS_SHADE: &str =
        include_str!("../tests/fixtures/dumpsys-window-displays-shade.txt");

    /// The captured policy dump with its keyguard flags rewritten, standing in
    /// for devices with a lock screen, which this capture does not cover.
    fn policy_with(showing: bool, secure: bool, trusted: bool) -> String {
        POLICY_NO_LOCK
            .replace("      showing=false", &format!("      showing={showing}"))
            .replace("      secure=false", &format!("      secure={secure}"))
            .replace(
                "        mTrusted=false",
                &format!("        mTrusted={trusted}"),
            )
    }

    #[test]
    fn reads_device_wakefulness_not_group_index() {
        assert_eq!(
            parse_wakefulness(POWER_DOZING).unwrap(),
            Wakefulness::Dozing
        );
        let awake = POWER_DOZING.replacen("mWakefulness=Dozing", "mWakefulness=Awake", 1);
        assert_eq!(parse_wakefulness(&awake).unwrap(), Wakefulness::Awake);
    }

    #[test]
    fn rejects_unknown_wakefulness() {
        let unknown = POWER_DOZING.replacen("mWakefulness=Dozing", "mWakefulness=Napping", 1);
        assert!(parse_wakefulness(&unknown).is_err());
    }

    #[test]
    fn reads_absent_keyguard() {
        assert_eq!(
            parse_keyguard(POLICY_NO_LOCK).unwrap(),
            Keyguard {
                showing: false,
                secure: false,
                trusted: false,
            }
        );
    }

    #[test]
    fn secure_keyguard_requires_credential_unless_trusted() {
        let locked = parse_keyguard(&policy_with(true, true, false)).unwrap();
        assert!(locked.showing && locked.requires_credential());

        let trusted = parse_keyguard(&policy_with(true, true, true)).unwrap();
        assert!(trusted.showing && !trusted.requires_credential());

        let swipe = parse_keyguard(&policy_with(true, false, false)).unwrap();
        assert!(swipe.showing && !swipe.requires_credential());
    }

    #[test]
    fn recognises_harness_focus() {
        let harness = parse_focused_window(DISPLAYS_HARNESS).unwrap();
        assert!(harness.belongs_to(ANDROID_HARNESS_PACKAGE));

        let shade = parse_focused_window(DISPLAYS_SHADE).unwrap();
        assert_eq!(shade, FocusedWindow::Window("NotificationShade".to_owned()));
        assert!(!shade.belongs_to(ANDROID_HARNESS_PACKAGE));

        let dialog = FocusedWindow::Window(ANDROID_HARNESS_PACKAGE.to_owned());
        assert!(dialog.belongs_to(ANDROID_HARNESS_PACKAGE));
        let other = FocusedWindow::Window(format!("{ANDROID_HARNESS_PACKAGE}.other/.Main"));
        assert!(!other.belongs_to(ANDROID_HARNESS_PACKAGE));
    }

    #[test]
    fn reads_absent_focus() {
        let none = DISPLAYS_SHADE.replace(
            "mCurrentFocus=Window{4ad28a7 u0 NotificationShade}",
            "mCurrentFocus=null",
        );
        assert_eq!(parse_focused_window(&none).unwrap(), FocusedWindow::None);
    }
}
