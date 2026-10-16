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
//!   installed; CI runs it against headless sway.
//!
//! A missing display server or tool fails the test with a message naming
//! what to install; it never passes without exercising PRIMARY.
//!
//! The ignored test checks that a Wayland session without data-control never
//! falls through to X11. It needs `WAYLAND_DISPLAY` at a compositor without a
//! data-control protocol (headless weston) and `DISPLAY` at an X server; CI
//! runs it with `--run-ignored only` against those two.
#![cfg(target_os = "linux")]

mod common;

use common::External;
use waterkit_clipboard::{ClipboardError, PrimarySelection};

const OWNED: &str = "waterkit-owned-primary";
const EXTERNAL: &str = "waterkit-external-primary";

/// Write PRIMARY through the crate and read it back through the crate and
/// through the display server's clipboard tool; then claim PRIMARY with that
/// tool and read it through the crate.
#[test]
fn primary_selection_round_trip() -> Result<(), ClipboardError> {
    let external = External::detect("PRIMARY");
    let mut primary = PrimarySelection::new()?;

    // Crate -> display server.
    primary.set_text(OWNED)?;
    let read = || futures::executor::block_on(primary.text());
    assert_eq!(read()?.as_deref(), Some(OWNED));
    assert_eq!(external.read(None), OWNED.as_bytes());

    // Display server -> crate.
    external.write(EXTERNAL.as_bytes(), None);
    let seeded = common::read_until(&Some(EXTERNAL.to_owned()), read)?;
    assert_eq!(seeded.as_deref(), Some(EXTERNAL));

    Ok(())
}

/// In a Wayland session whose compositor offers no data-control protocol,
/// creating the handle fails with the documented error even though an X
/// server is reachable through `DISPLAY`.
#[test]
#[ignore = "needs WAYLAND_DISPLAY at a compositor without data-control and DISPLAY at an X server"]
fn wayland_without_data_control_never_uses_x11() {
    common::assert_wayland_without_data_control_fails("PRIMARY", PrimarySelection::new());
}
