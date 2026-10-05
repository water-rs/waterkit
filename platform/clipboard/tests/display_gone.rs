//! Linux CLIPBOARD queries against an X server that has gone away.
//!
//! A query whose display server is gone must fail, never read as an empty
//! clipboard. The test starts an X server of its own with `Xvfb` (package
//! `xvfb`), so stopping it disturbs no other test, and runs the clipboard side
//! in a child process of this test binary whose `DISPLAY` names that server:
//! the crate chooses its display server from the process environment.
//!
//! A missing `Xvfb` fails the test with a message naming the package; it never
//! passes without stopping a server under a live handle.
#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::process::{Child, ChildStdout, Command, Stdio};

use waterkit_clipboard::{Clipboard, ClipboardError};

/// The child's test, which the parent runs in a process of its own.
const CHILD_TEST: &str = "queries_against_a_stopped_x_server";
/// Set by the parent in the child's environment, so the child knows it was
/// started by the parent rather than by a run of the ignored tests.
const PARENT_MARKER: &str = "WATERKIT_CLIPBOARD_TEST_PRIVATE_X_SERVER";
/// The line the child writes once its handle is connected.
const READY: &str = "waterkit-clipboard: handle connected";

/// A query of whether the clipboard offers a kind of content.
type Query = fn(&Clipboard) -> Result<bool, ClipboardError>;

/// The queries a stopped X server must fail.
const QUERIES: [(&str, Query); 4] = [
    ("has_text", Clipboard::has_text),
    ("has_html", Clipboard::has_html),
    ("has_files", Clipboard::has_files),
    ("has_image", Clipboard::has_image),
];

/// Start a private X server, connect a handle to it in a child process, stop
/// the server, and have the child query the handle.
#[test]
fn has_queries_fail_once_the_x_server_is_gone() {
    let (mut xvfb, display) = start_xvfb();
    let mut child = Command::new(std::env::current_exe().expect("the test binary's path"))
        .args(["--exact", CHILD_TEST, "--ignored"])
        .env("DISPLAY", &display)
        .env(PARENT_MARKER, &display)
        .env_remove("WAYLAND_DISPLAY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start the test binary as the clipboard child");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));
    if !wait_for_ready(&mut stdout) {
        stop(&mut xvfb);
        panic!(
            "the child exited before connecting its handle to the X server\n{}",
            child_output(child, stdout)
        );
    }

    stop(&mut xvfb);
    // The child queries the handle once this line arrives.
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(b"stopped\n")
        .expect("failed to tell the child the X server stopped");
    let status = child
        .wait()
        .expect("failed to wait for the clipboard child");
    assert!(
        status.success(),
        "the clipboard child failed with {status}\n{}",
        child_output(child, stdout)
    );
}

/// The clipboard side, run by [`has_queries_fail_once_the_x_server_is_gone`]
/// against the X server it stops.
#[test]
#[ignore = "run by has_queries_fail_once_the_x_server_is_gone in a process of its own"]
fn queries_against_a_stopped_x_server() {
    assert!(
        std::env::var_os(PARENT_MARKER).is_some(),
        "{CHILD_TEST} is the child of has_queries_fail_once_the_x_server_is_gone, which starts \
         and stops its X server; run that test instead"
    );
    let clipboard = Clipboard::new().expect("failed to connect to the test's X server");
    for (name, query) in QUERIES {
        let offered = query(&clipboard)
            .unwrap_or_else(|error| panic!("{name} failed while the X server ran: {error:?}"));
        assert!(
            !offered,
            "{name} on a fresh X server, whose CLIPBOARD has no owner"
        );
    }

    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{READY}").expect("failed to tell the parent the handle is connected");
    stdout
        .flush()
        .expect("failed to tell the parent the handle is connected");
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .expect("failed to wait for the parent to stop the X server");
    assert_eq!(line, "stopped\n", "the parent did not stop the X server");

    for (name, query) in QUERIES {
        match query(&clipboard) {
            Err(ClipboardError::Platform(_)) => {}
            other => panic!("{name} against a stopped X server returned {other:?}"),
        }
    }
}

/// Start `Xvfb` on a free display and return it with that display's name,
/// once it accepts connections.
fn start_xvfb() -> (Child, String) {
    // `-displayfd` writes the display number once the server accepts
    // connections; fd 1 is the piped stdout, which Xvfb does not log to.
    let mut xvfb = Command::new("Xvfb")
        .args(["-displayfd", "1", "-nolisten", "tcp"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|error| match error.kind() {
            ErrorKind::NotFound => panic!(
                "this test stops an X server of its own, started with `Xvfb`, which is not \
                 installed; install the `xvfb` package"
            ),
            _ => panic!("failed to start `Xvfb`: {error}"),
        });
    let mut number = String::new();
    BufReader::new(xvfb.stdout.take().expect("stdout is piped"))
        .read_line(&mut number)
        .expect("failed to read the display number from `Xvfb`");
    let number = number.trim();
    if number.is_empty() {
        let status = xvfb.wait().expect("failed to wait for `Xvfb`");
        panic!("`Xvfb` exited with {status} before reporting its display");
    }
    (xvfb, format!(":{number}"))
}

/// Stop the X server and wait until it has exited, which closes every client
/// connection.
fn stop(xvfb: &mut Child) {
    xvfb.kill().expect("failed to stop `Xvfb`");
    xvfb.wait().expect("failed to wait for `Xvfb` to exit");
}

/// Read the child's output until it reports a connected handle; `false` when
/// it exits first.
fn wait_for_ready(stdout: &mut BufReader<ChildStdout>) -> bool {
    let mut line = String::new();
    loop {
        line.clear();
        match stdout.read_line(&mut line) {
            Ok(0) => return false,
            Ok(_) if line.trim_end() == READY => return true,
            Ok(_) => {}
            Err(error) => panic!("failed to read the clipboard child's output: {error}"),
        }
    }
}

/// What the child wrote, for a failure message. The child has exited or is
/// about to.
fn child_output(mut child: Child, mut stdout: BufReader<ChildStdout>) -> String {
    let mut out = String::new();
    let mut err = String::new();
    // Partial output still explains the failure, so read errors are shown in
    // place of what could not be read.
    if let Err(error) = stdout.read_to_string(&mut out) {
        out = format!("<unreadable: {error}>");
    }
    if let Some(mut stderr) = child.stderr.take()
        && let Err(error) = stderr.read_to_string(&mut err)
    {
        err = format!("<unreadable: {error}>");
    }
    // Reap the child; its status was already reported or is not the point.
    let _ = child.wait();
    format!("--- child stdout ---\n{out}\n--- child stderr ---\n{err}")
}
