//! Test notification with actions, quick reply, and updates in a bundled macOS app.

#[cfg(target_os = "macos")]
use std::fs::OpenOptions;
#[cfg(target_os = "macos")]
use std::io::Write;
#[cfg(target_os = "macos")]
use waterkit_notification::{Action, Notification, TextInputAction};

#[cfg(target_os = "macos")]
fn log(msg: &str) {
    // Write to a fixed log path relative to the executable
    let log_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .map(|p| p.join("../../../notification-test.log"))
        .unwrap_or_else(|| "/tmp/notification-test.log".into());

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

#[cfg(target_os = "macos")]
fn run_loop_for(seconds: f64) {
    let iterations = (seconds * 10.0) as u32;
    for _ in 0..iterations {
        unsafe {
            let mode = core_foundation_sys::runloop::kCFRunLoopDefaultMode;
            CFRunLoopRunInMode(mode.cast(), 0.1, false);
        }
    }
}

#[cfg(target_os = "macos")]
fn main() {
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

    run_loop_for(5.0);

    // Test 2: Notification update using handle
    log("\n=== Test 2: Notification Update ===");
    log("Simulating download progress...");

    // Show initial notification and keep the handle
    let handle = match Notification::new()
        .title("Downloading file.zip")
        .body("0% complete")
        .subtitle("Update test")
        .show()
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

    run_loop_for(0.5);

    // Update using the handle
    for progress in (20..=100).step_by(20) {
        match handle
            .update()
            .title("Downloading file.zip")
            .body(format!("{progress}% complete"))
            .subtitle("Update test")
            .show()
        {
            Ok(_) => log(&format!("Updated to {progress}%")),
            Err(e) => {
                log(&format!("Failed to update: {e}"));
                return;
            }
        }
        run_loop_for(0.5);
    }

    // Final update with action
    match handle
        .update()
        .title("Download Complete!")
        .body("file.zip is ready")
        .subtitle("Update test")
        .action(Action::new("Open", "https://waterui.dev"))
        .show()
    {
        Ok(_) => log("Download complete notification sent!"),
        Err(e) => log(&format!("Failed to send final notification: {e}")),
    }

    log("\nWaiting 10 seconds for interactions...");
    run_loop_for(10.0);

    log("Test complete.");
}

#[cfg(not(target_os = "macos"))]
fn main() {}
