//! macOS structured test for `waterkit-menu`.
//!
//! The harness only runs on macOS, so the `AppKit` stack it drives is scoped to
//! macOS rather than built for every target the workspace is swept with.
//! Building the workspace for another target still produces this binary, it
//! just has nothing to do.

#[cfg(target_os = "macos")]
mod harness;

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    harness::main()
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("waterkit-menu-test is a macOS-only test harness; nothing to run on this target.");
}
