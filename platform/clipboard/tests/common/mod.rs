//! What the Linux selection tests share: the display server's clipboard CLI,
//! which checks a selection from outside the crate, and the Wayland error
//! path.

use std::ffi::OsString;
use std::io::{ErrorKind, Write};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use waterkit_clipboard::ClipboardError;

/// How long the crate may take to observe a selection claimed by the external
/// tool. The tools fork a daemon that claims the selection after the parent
/// exits, so the claim lands shortly after the write command returns.
const EXTERNAL_CLAIM_DEADLINE: Duration = Duration::from_secs(2);

/// The display server the crate talks to, and the CLI that cross-checks it
/// from outside the crate. Chosen the same way the crate's backend is: a
/// Wayland compositor when `WAYLAND_DISPLAY` is set, X11 otherwise.
pub struct External {
    tool: Tool,
    /// The selection's X11 name, `CLIPBOARD` or `PRIMARY`.
    selection: &'static str,
}

enum Tool {
    /// `xclip` on X11.
    Xclip,
    /// `wl-copy`/`wl-paste` on Wayland.
    WlClipboard,
}

impl External {
    /// The tool for `selection`, `"CLIPBOARD"` or `"PRIMARY"`.
    pub fn detect(selection: &'static str) -> Self {
        assert!(
            matches!(selection, "CLIPBOARD" | "PRIMARY"),
            "{selection} is not a Linux selection"
        );
        let tool = if display_variable("WAYLAND_DISPLAY").is_some() {
            Tool::WlClipboard
        } else if display_variable("DISPLAY").is_some() {
            Tool::Xclip
        } else {
            panic!(
                "the selection round trip needs a display server: set DISPLAY to an X server \
                 (without a desktop session, install `xvfb` and run the tests under `xvfb-run`) \
                 or WAYLAND_DISPLAY to a compositor with a data-control protocol"
            );
        };
        Self { tool, selection }
    }

    /// The distribution package that ships the cross-check tool.
    const fn package(&self) -> &'static str {
        match self.tool {
            Tool::Xclip => "xclip",
            Tool::WlClipboard => "wl-clipboard",
        }
    }

    /// The command reading or writing this selection, as `mime` when given:
    /// `xclip` with `xclip_direction`, or `wl_program`.
    fn command(&self, xclip_direction: &str, wl_program: &str, mime: Option<&str>) -> Command {
        match self.tool {
            Tool::Xclip => {
                let mut command = Command::new("xclip");
                let selection = self.selection.to_lowercase();
                command.args(["-selection", &selection, xclip_direction]);
                if let Some(mime) = mime {
                    command.args(["-t", mime]);
                }
                command
            }
            Tool::WlClipboard => {
                let mut command = Command::new(wl_program);
                if self.selection == "PRIMARY" {
                    command.arg("--primary");
                }
                if let Some(mime) = mime {
                    command.args(["--type", mime]);
                }
                command
            }
        }
    }

    /// Read the selection through the external tool, as `mime` when given
    /// and as text otherwise.
    pub fn read(&self, mime: Option<&str>) -> Vec<u8> {
        let mut command = self.command("-o", "wl-paste", mime);
        if matches!(self.tool, Tool::WlClipboard) {
            command.arg("--no-newline");
        }
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
        stdout
    }

    /// Claim the selection with `bytes` through the external tool, offered as
    /// `mime` when given and as text otherwise.
    pub fn write(&self, bytes: &[u8], mime: Option<&str>) {
        let mut command = self.command("-i", "wl-copy", mime);
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
            .write_all(bytes)
            .expect("failed to pass the data to the external clipboard tool");
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
                    "the selection round trip cross-checks the selection with `{program}`, which \
                     is not installed; install the `{}` package",
                    self.package()
                ),
                _ => panic!("failed to run `{program}`: {error}"),
            }
        })
    }
}

/// The value of a display-server variable, with an empty value counting as
/// unset as it does for the crate.
pub fn display_variable(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

/// Call `read` until it returns `expected` or the deadline for a claim by the
/// external tool passes, and return the last value read.
pub fn read_until<T: PartialEq>(
    expected: &T,
    mut read: impl FnMut() -> Result<T, ClipboardError>,
) -> Result<T, ClipboardError> {
    let deadline = Instant::now() + EXTERNAL_CLAIM_DEADLINE;
    loop {
        let value = read()?;
        if value == *expected || Instant::now() >= deadline {
            return Ok(value);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Assert that creating a handle in a Wayland session without data-control
/// failed with the documented error naming `selection`, although an X server
/// is reachable through `DISPLAY`.
pub fn assert_wayland_without_data_control_fails<T>(
    selection: &str,
    created: Result<T, ClipboardError>,
) {
    for name in ["WAYLAND_DISPLAY", "DISPLAY"] {
        assert!(
            display_variable(name).is_some(),
            "wayland_without_data_control_never_uses_x11 needs {name}: WAYLAND_DISPLAY at a \
             compositor without data-control (such as headless weston) and DISPLAY at an X server"
        );
    }
    match created {
        Err(ClipboardError::Platform(reason)) => assert!(
            reason.starts_with(selection)
                && reason.contains("Wayland session")
                && reason.contains("data-control"),
            "the error does not name {selection} and the missing data-control protocol: {reason}"
        ),
        Err(error) => panic!("expected ClipboardError::Platform, got {error:?}"),
        Ok(_) => {
            panic!("the {selection} handle was created in a Wayland session without data-control")
        }
    }
}
