//! macOS test binary for waterkit-location.
//!
//! Run with: ./tests/macos/location/run.sh

use std::io::Write;
use waterkit_location::Location;
use waterkit_permission::{Permission, PermissionStatus};

/// Output helper that writes to both stdout and a log file
struct Output {
    file: Option<std::fs::File>,
}

impl Output {
    fn new() -> Self {
        // Try to create log file next to the executable
        let file = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.join("../../../location-test.log")))
            .and_then(|p| std::fs::File::create(p).ok());
        Self { file }
    }

    fn log(&mut self, msg: &str) {
        println!("{}", msg);
        if let Some(ref mut f) = self.file {
            let _ = writeln!(f, "{}", msg);
            let _ = f.flush();
        }
    }
}

#[tokio::main]
async fn main() {
    let mut out = Output::new();

    out.log("=== Waterkit Location Test (macOS) ===\n");

    // Check permission status
    out.log("Checking location permission...");
    let status = waterkit_permission::check(Permission::Location).await;
    out.log(&format!("Permission status: {:?}", status));

    match status {
        PermissionStatus::Granted => {
            out.log("  ✓ Location access granted\n");
        }
        PermissionStatus::NotDetermined => {
            out.log("  ⚠ Location permission not determined\n");

            out.log("Requesting location permission...");
            match waterkit_permission::request(Permission::Location).await {
                Ok(new_status) => {
                    out.log(&format!("New permission status: {:?}", new_status));
                    if new_status != PermissionStatus::Granted {
                        out.log("  → Permission was not granted\n");
                    } else {
                        out.log("  ✓ Permission granted!\n");
                    }
                }
                Err(e) => {
                    out.log(&format!("Permission request failed: {}\n", e));
                    return;
                }
            }
        }
        PermissionStatus::Denied => {
            out.log("  ✗ Location access denied");
            out.log("  → Go to System Settings > Privacy & Security > Location Services");
            out.log("    and enable LocationTest\n");
            return;
        }
        PermissionStatus::Restricted => {
            out.log("  ✗ Location access restricted by system policy\n");
            return;
        }
        unexpected => {
            panic!("waterkit-location-test: unsupported permission status {unexpected:?}");
        }
    }

    // Get location
    out.log("Getting current location...");
    match Location::get().await {
        Ok(location) => {
            out.log("✓ Location retrieved successfully!");
            out.log(&format!("  Latitude:  {:.6}°", location.latitude()));
            out.log(&format!("  Longitude: {:.6}°", location.longitude()));
            if let Some(alt) = location.altitude() {
                out.log(&format!("  Altitude:  {:.1}m", alt));
            }
            if let Some(acc) = location.horizontal_accuracy() {
                out.log(&format!("  Accuracy:  {:.1}m", acc));
            }
            out.log(&format!("  Timestamp: {:?}", location.timestamp()));
        }
        Err(e) => {
            out.log(&format!("✗ Failed to get location: {}", e));
        }
    }
}
