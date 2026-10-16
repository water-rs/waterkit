//! Test notification with actions, quick reply, and updates in a bundled macOS app.

#[cfg(target_os = "macos")]
use std::fs::OpenOptions;
#[cfg(target_os = "macos")]
use std::io::Write;
#[cfg(target_os = "macos")]
use std::time::Duration;
#[cfg(target_os = "macos")]
use waterkit_notification::{Action, Notification, TextInputAction};

#[cfg(target_os = "macos")]
fn log(msg: &str) {
    // Write to a fixed log path relative to the executable
    let executable = std::env::current_exe().expect("the harness executable has a path");
    let log_path = executable
        .parent()
        .expect("the harness executable lives in a directory")
        .join("../../../notification-test.log");

    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) {
        let _ = writeln!(file, "{msg}");
    }
    tracing::debug!("{msg}");
}

#[cfg(target_os = "macos")]
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRunLoopRunInMode(
        mode: *const std::ffi::c_void,
        seconds: f64,
        returnAfterSourceHandled: bool,
    ) -> i32;
}

/// Runs the main run loop for `duration`, so notification callbacks are
/// delivered while the harness waits for interactions.
#[cfg(target_os = "macos")]
fn run_loop_for(duration: Duration) {
    const SLICE: Duration = Duration::from_millis(100);
    for _ in 0..duration.as_millis() / SLICE.as_millis() {
        // SAFETY: `kCFRunLoopDefaultMode` is a constant CoreFoundation string
        // that lives for the whole process, and running the current thread's
        // run loop has no other precondition.
        unsafe {
            let mode = core_foundation_sys::runloop::kCFRunLoopDefaultMode;
            CFRunLoopRunInMode(mode.cast(), SLICE.as_secs_f64(), false);
        }
    }
}

#[cfg(target_os = "macos")]
#[tokio::main]
async fn main() {
    // Test 1: Notification with quick reply
    log("=== Test 1: Quick Reply ===");
    log("Sending notification with quick reply...");

    match Notification::new()
        .title("New Message from WaterKit")
        .body("Hey, how are you?")
        .subtitle("Quick reply test")
        .text_input_action(
            TextInputAction::new("reply", "Reply")
                .placeholder("Type a message...")
                .submit_label("Send"),
        )
        .action(Action::new("View", "https://waterui.dev"))
        .show()
        .await
    {
        Ok(_handle) => {
            log("Notification sent!");
            log("Try clicking 'Reply' to test quick reply...");
        }
        Err(e) => {
            log(&format!("Failed to send notification: {e}"));
            return;
        }
    }

    run_loop_for(Duration::from_secs(5));

    // Test 2: Notification update using handle
    log("\n=== Test 2: Notification Update ===");
    log("Simulating download progress...");

    // Show initial notification and keep the handle
    let handle = match Notification::new()
        .title("Downloading file.zip")
        .body("0% complete")
        .subtitle("Update test")
        .show()
        .await
    {
        Ok(h) => {
            log("Initial notification sent (0%)");
            h
        }
        Err(e) => {
            log(&format!("Failed to send initial notification: {e}"));
            return;
        }
    };

    run_loop_for(Duration::from_millis(500));

    // Update using the handle
    for progress in (20..=100).step_by(20) {
        match handle
            .update()
            .title("Downloading file.zip")
            .body(format!("{progress}% complete"))
            .subtitle("Update test")
            .show()
            .await
        {
            Ok(_) => log(&format!("Updated to {progress}%")),
            Err(e) => {
                log(&format!("Failed to update: {e}"));
                return;
            }
        }
        run_loop_for(Duration::from_millis(500));
    }

    // Final update with action
    match handle
        .update()
        .title("Download Complete!")
        .body("file.zip is ready")
        .subtitle("Update test")
        .action(Action::new("Open", "https://waterui.dev"))
        .show()
        .await
    {
        Ok(_) => log("Download complete notification sent!"),
        Err(e) => log(&format!("Failed to send final notification: {e}")),
    }

    log("\nWaiting 10 seconds for interactions...");
    run_loop_for(Duration::from_secs(10));

    log("Test complete.");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("waterkit-notification-test is a macOS-only harness; nothing to run on this target.");
}
