//! Basic notification example.

use waterkit_notification::Notification;

fn main() -> Result<(), waterkit_notification::NotificationError> {
    println!("Sending notification...");

    Notification::new()
        .title("Hello")
        .body("World from WaterKit!")
        .show()?;

    println!("Notification sent.");
    Ok(())
}
