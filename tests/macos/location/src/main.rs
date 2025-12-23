//! macOS test binary for waterkit-location.
//!
//! Run with: cargo run -p waterkit-location-test

use waterkit_location::{Location, LocationError, LocationManager};
use waterkit_permission::{Permission, PermissionStatus};

#[tokio::main]
async fn main() {
    println!("=== Waterkit Location Test (macOS) ===\n");

    // Check permission status
    println!("Checking location permission...");
    let status = waterkit_permission::check(Permission::Location).await;
    println!("Permission status: {:?}\n", status);

    if status != PermissionStatus::Granted {
        println!("Requesting location permission...");
        match waterkit_permission::request(Permission::Location).await {
            Ok(new_status) => println!("New permission status: {:?}\n", new_status),
            Err(e) => {
                println!("Permission request failed: {}\n", e);
                return;
            }
        }
    }

    // Get location
    println!("Getting current location...");
    match LocationManager::get_location().await {
        Ok(location) => {
            println!("✓ Location retrieved successfully!");
            println!("  Latitude:  {:.6}°", location.latitude);
            println!("  Longitude: {:.6}°", location.longitude);
            if let Some(alt) = location.altitude {
                println!("  Altitude:  {:.1}m", alt);
            }
            if let Some(acc) = location.horizontal_accuracy {
                println!("  Accuracy:  {:.1}m", acc);
            }
            println!("  Timestamp: {}", location.timestamp);
        }
        Err(e) => {
            println!("✗ Failed to get location: {}", e);
        }
    }
}
