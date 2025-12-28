//! Test notification with actions in a bundled macOS app.

use std::fs::OpenOptions;
use std::io::Write;
use waterkit_notification::{Action, Notification};

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
    // Also print to stdout
    println!("{msg}");
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRunLoopRunInMode(
        mode: *const std::ffi::c_void,
        seconds: f64,
        returnAfterSourceHandled: bool,
    ) -> i32;
}

fn main() {
    log("Sending notification with action buttons...");

    match Notification::new()
        .title("WaterKit Notification Test")
        .body("Click an action button to open the URL")
        .subtitle("From bundled app")
        .action(Action::new("Visit WaterUI", "https://waterui.dev"))
        .action(Action::new("Dismiss", "waterui://dismiss"))
        .show()
    {
        Ok(_) => {
            log("Notification sent successfully!");
            log("Click an action button within 30 seconds...");
        }
        Err(e) => {
            log(&format!("Failed to send notification: {e}"));
            return;
        }
    }

    // Run the CFRunLoop to process notification callbacks
    // This allows the UNUserNotificationCenterDelegate to receive action events
    log("Running event loop for 30 seconds...");
    for _ in 0..300 {
        // Run loop for 0.1 seconds at a time
        unsafe {
            // kCFRunLoopDefaultMode
            let mode = core_foundation_sys::runloop::kCFRunLoopDefaultMode;
            CFRunLoopRunInMode(mode.cast(), 0.1, false);
        }
    }

    log("Test complete.");
}
