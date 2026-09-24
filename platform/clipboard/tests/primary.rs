//! Linux PRIMARY selection round-trip tests.
//!
//! These are no-ops on headless machines (CI runs without a display). Run
//! under `xvfb-run` for X11 coverage, or inside a Wayland session for
//! data-control coverage.
#![cfg(target_os = "linux")]

use std::process::{Command, Stdio};

use waterkit_clipboard::{ClipboardError, PrimarySelection};

const OWNED: &str = "waterkit-owned-primary";
const EXTERNAL: &str = "waterkit-external-primary";

/// A display server to talk to, and the CLI used to cross-check it from
/// outside the crate.
enum External {
    /// `xclip` on X11.
    Xclip,
    /// `wl-copy`/`wl-paste` on Wayland.
    WlClipboard,
}

fn external() -> Option<External> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        Some(External::WlClipboard)
    } else if std::env::var_os("DISPLAY").is_some() {
        Some(External::Xclip)
    } else {
        None
    }
}

fn external_read(ext: &External) -> Option<String> {
    let output = match ext {
        External::Xclip => Command::new("xclip")
            .args(["-selection", "primary", "-o"])
            .output(),
        External::WlClipboard => Command::new("wl-paste").arg("--primary").output(),
    }
    .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn external_write(ext: &External, text: &str) -> bool {
    match ext {
        // Both helpers daemonize to keep serving the selection; detach all
        // stdio so the forked server does not hold the test's output pipes.
        External::Xclip => Command::new("xclip")
            .args(["-selection", "primary", "-i"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child
                    .stdin
                    .as_mut()
                    .expect("stdin piped")
                    .write_all(text.as_bytes())
                    .and_then(|()| child.wait())
                    .map(|status| status.success())
            })
            .unwrap_or(false),
        External::WlClipboard => Command::new("wl-copy")
            .arg("--primary")
            .arg(text)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success()),
    }
}

/// Read PRIMARY through the crate until it matches `expected` or the
/// deadline passes. The external helpers daemonize before claiming the
/// selection, so a plain single read races them.
fn read_until(
    primary: &PrimarySelection,
    expected: &str,
) -> Result<Option<String>, ClipboardError> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let text = futures::executor::block_on(primary.text())?;
        if text.as_deref() == Some(expected) || std::time::Instant::now() >= deadline {
            return Ok(text);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Write PRIMARY through the crate, then read it back both through the crate
/// and through the display server's own clipboard tool. Then seed PRIMARY
/// with that tool and read it through the crate.
#[test]
fn primary_selection_round_trip() -> Result<(), ClipboardError> {
    let Some(ext) = external() else {
        eprintln!("skipping: no X11 display or Wayland compositor available");
        return Ok(());
    };

    let mut primary = PrimarySelection::new()?;

    // Crate -> display server.
    primary.set_text(OWNED)?;
    let written = futures::executor::block_on(primary.text())?;
    assert_eq!(written.as_deref(), Some(OWNED));
    if let Some(readback) = external_read(&ext) {
        assert_eq!(readback.trim_end_matches('\n'), OWNED);
    } else {
        eprintln!("external clipboard tool missing; skipped cross-check");
    }

    // Display server -> crate.
    if external_write(&ext, EXTERNAL) {
        let seeded = read_until(&primary, EXTERNAL)?;
        assert_eq!(seeded.as_deref(), Some(EXTERNAL));
    } else {
        eprintln!("external clipboard tool missing; skipped seeding");
    }

    Ok(())
}
