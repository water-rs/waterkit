//! Linux PRIMARY selection tests.
//!
//! The round trip needs a display server and that display server's clipboard
//! CLI, which it uses to check the selection from outside the crate:
//!
//! - X11: `DISPLAY` pointing at an X server and `xclip` installed. On a
//!   machine without a desktop session, run the suite under `xvfb-run`
//!   (package `xvfb`); CI starts an Xvfb server for it.
//! - Wayland: `WAYLAND_DISPLAY` pointing at a compositor that offers a
//!   data-control protocol with a primary selection, and `wl-clipboard`
//!   installed.
//!
//! A missing display server or tool fails the test with a message naming
//! what to install; it never passes without exercising PRIMARY.
//!
//! The ignored test checks that a Wayland session without data-control never
//! falls through to X11. It needs `WAYLAND_DISPLAY` at a compositor without a
//! data-control protocol (headless weston) and `DISPLAY` at an X server; CI
//! runs it with `--run-ignored only` against those two.
#![cfg(target_os = "linux")]

use std::ffi::OsString;
use std::io::{ErrorKind, Write};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use waterkit_clipboard::{ClipboardError, PrimarySelection};

const OWNED: &str = "waterkit-owned-primary";
const EXTERNAL: &str = "waterkit-external-primary";

/// How long the crate may take to observe a selection claimed by the external
/// tool. The tools fork a daemon that claims PRIMARY after the parent exits,
/// so the claim lands shortly after the write command returns.
const EXTERNAL_CLAIM_DEADLINE: Duration = Duration::from_secs(2);

/// The display server the crate talks to, and the CLI that cross-checks it
/// from outside the crate. Chosen the same way the crate's backend is: a
/// Wayland compositor when `WAYLAND_DISPLAY` is set, X11 otherwise.
enum External {
    /// `xclip` on X11.
    Xclip,
    /// `wl-copy`/`wl-paste` on Wayland.
    WlClipboard,
}

impl External {
    fn detect() -> Self {
        if display_variable("WAYLAND_DISPLAY").is_some() {
            Self::WlClipboard
        } else if display_variable("DISPLAY").is_some() {
            Self::Xclip
        } else {
            panic!(
                "primary_selection_round_trip needs a display server: set DISPLAY to an X server \
                 (without a desktop session, install `xvfb` and run the tests under `xvfb-run`) \
                 or WAYLAND_DISPLAY to a compositor with a data-control protocol"
            );
        }
    }

    /// The distribution package that ships the cross-check tool.
    const fn package(&self) -> &'static str {
        match self {
            Self::Xclip => "xclip",
            Self::WlClipboard => "wl-clipboard",
        }
    }

    fn read_command(&self) -> Command {
        match self {
            Self::Xclip => {
                let mut command = Command::new("xclip");
                command.args(["-selection", "primary", "-o"]);
                command
            }
            Self::WlClipboard => {
                let mut command = Command::new("wl-paste");
                command.args(["--primary", "--no-newline"]);
                command
            }
        }
    }

    /// The command that takes the text on stdin and claims PRIMARY with it.
    fn write_command(&self) -> Command {
        match self {
            Self::Xclip => {
                let mut command = Command::new("xclip");
                command.args(["-selection", "primary", "-i"]);
                command
            }
            Self::WlClipboard => {
                let mut command = Command::new("wl-copy");
                command.arg("--primary");
                command
            }
        }
    }

    /// Read PRIMARY through the external tool.
    fn read(&self) -> String {
        let mut command = self.read_command();
        let Output {
            status,
            stdout,
            stderr,
        } = self.run(command.stdin(Stdio::null()), Command::output);
        assert!(
            status.success(),
            "`{}` failed with {status}: {}",
            command.get_program().display(),
            String::from_utf8_lossy(&stderr)
        );
        String::from_utf8(stdout).expect("PRIMARY text read by the external tool is not UTF-8")
    }

    /// Claim PRIMARY with `text` through the external tool.
    fn write(&self, text: &str) {
        let mut command = self.write_command();
        // Both tools fork a daemon that keeps serving the selection; it
        // inherits the stdio, so detach it from the test's output pipes.
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = self.run(&mut command, Command::spawn);
        child
            .stdin
            .as_mut()
            .expect("stdin is piped")
            .write_all(text.as_bytes())
            .expect("failed to pass the text to the external clipboard tool");
        // `wait` closes stdin first, which is the end of input the tool waits
        // for before claiming the selection.
        let status = child
            .wait()
            .expect("failed to wait for the external clipboard tool");
        assert!(
            status.success(),
            "`{}` failed with {status}",
            command.get_program().display()
        );
    }

    /// Run `command`, turning a missing executable into an instruction to
    /// install the package that ships it.
    fn run<T>(
        &self,
        command: &mut Command,
        run: impl FnOnce(&mut Command) -> std::io::Result<T>,
    ) -> T {
        run(command).unwrap_or_else(|error| {
            let program = command.get_program().display();
            match error.kind() {
                ErrorKind::NotFound => panic!(
                    "primary_selection_round_trip cross-checks PRIMARY with `{program}`, which is \
                     not installed; install the `{}` package",
                    self.package()
                ),
                _ => panic!("failed to run `{program}`: {error}"),
            }
        })
    }
}

/// The value of a display-server variable, with an empty value counting as
/// unset as it does for the crate.
fn display_variable(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

/// Read PRIMARY through the crate until it holds `expected` or the deadline
/// passes, and return the last text read.
fn read_until(
    primary: &PrimarySelection,
    expected: &str,
) -> Result<Option<String>, ClipboardError> {
    let deadline = Instant::now() + EXTERNAL_CLAIM_DEADLINE;
    loop {
        let text = futures::executor::block_on(primary.text())?;
        if text.as_deref() == Some(expected) || Instant::now() >= deadline {
            return Ok(text);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Write PRIMARY through the crate and read it back through the crate and
/// through the display server's clipboard tool; then claim PRIMARY with that
/// tool and read it through the crate.
#[test]
fn primary_selection_round_trip() -> Result<(), ClipboardError> {
    let external = External::detect();
    let mut primary = PrimarySelection::new()?;

    // Crate -> display server.
    primary.set_text(OWNED)?;
    let written = futures::executor::block_on(primary.text())?;
    assert_eq!(written.as_deref(), Some(OWNED));
    assert_eq!(external.read(), OWNED);

    // Display server -> crate.
    external.write(EXTERNAL);
    let seeded = read_until(&primary, EXTERNAL)?;
    assert_eq!(seeded.as_deref(), Some(EXTERNAL));

    Ok(())
}

/// In a Wayland session whose compositor offers no data-control protocol,
/// creating the handle fails with the documented error even though an X
/// server is reachable through `DISPLAY`.
#[test]
#[ignore = "needs WAYLAND_DISPLAY at a compositor without data-control and DISPLAY at an X server"]
fn wayland_without_data_control_never_uses_x11() {
    for name in ["WAYLAND_DISPLAY", "DISPLAY"] {
        assert!(
            display_variable(name).is_some(),
            "wayland_without_data_control_never_uses_x11 needs {name}: WAYLAND_DISPLAY at a \
             compositor without data-control (such as headless weston) and DISPLAY at an X server"
        );
    }
    match PrimarySelection::new() {
        Err(ClipboardError::Platform(reason)) => assert!(
            reason.contains("Wayland session") && reason.contains("data-control"),
            "the error does not name the missing data-control protocol: {reason}"
        ),
        Err(error) => panic!("expected ClipboardError::Platform, got {error:?}"),
        Ok(_) => {
            panic!("PrimarySelection::new succeeded in a Wayland session without data-control")
        }
    }
}
