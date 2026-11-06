//! The macOS grant status read end to end: this binary's `main` hands
//! helper launches to the entry the way an app does, then reads each grant
//! through a fresh helper instance of itself. Only status is read, which
//! never prompts.

use std::process::ExitCode;

#[cfg(target_os = "macos")]
fn main() -> ExitCode {
    use waterkit_permission::{Permission, PermissionStatus, check};

    if let Some(code) = waterkit_permission::run_helper_entry_if_requested() {
        return code;
    }
    let (screen_recording, accessibility, direct_capture) = futures::executor::block_on(async {
        futures::join!(
            check(Permission::ScreenRecording),
            check(Permission::Accessibility),
            check(Permission::DirectScreenCapture),
        )
    });
    for status in [screen_recording, accessibility] {
        assert!(
            matches!(
                status,
                PermissionStatus::Granted | PermissionStatus::NotGranted
            ),
            "a TCC grant reads granted or not granted, got {status:?}"
        );
    }
    let expected_direct_capture = if screen_recording == PermissionStatus::Granted {
        PermissionStatus::Unknown
    } else {
        PermissionStatus::NotGranted
    };
    assert_eq!(direct_capture, expected_direct_capture);
    println!(
        "helper_entry: screen recording {screen_recording:?}, accessibility {accessibility:?}, \
         direct capture {direct_capture:?}"
    );
    ExitCode::SUCCESS
}

#[cfg(not(target_os = "macos"))]
fn main() -> ExitCode {
    assert!(waterkit_permission::run_helper_entry_if_requested().is_none());
    ExitCode::SUCCESS
}
