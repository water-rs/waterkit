//! Notification with URL action buttons.
//!
//! Actions open URLs or deep links when clicked.

use waterkit_notification::{Action, Notification};

fn main() -> Result<(), waterkit_notification::NotificationError> {
    println!("Showing notification with action buttons...");

    Notification::new()
        .title("WaterUI Update Available")
        .body("A new version of WaterUI is ready to download")
        .action(Action::new("View Details", "https://waterui.dev"))
        .action(Action::new("Later", "waterui://dismiss"))
        .show()?;

    println!("Notification sent!");
    println!("Click an action button to open the URL.");

    // Keep the program running briefly to see the notification
    std::thread::sleep(std::time::Duration::from_secs(5));

    Ok(())
}
