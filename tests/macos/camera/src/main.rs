//! macOS camera preview harness.
//!
//! This is a hand-run harness for a macOS window, so the window/GPU stack it
//! needs is only depended on - and only compiled - for macOS. Building the
//! workspace for another target still produces this binary, it just has nothing
//! to do.

#[cfg(target_os = "macos")]
mod preview;

#[cfg(target_os = "macos")]
fn main() {
    preview::run();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("waterkit-camera-test is a macOS-only preview harness; nothing to run on this target.");
}
