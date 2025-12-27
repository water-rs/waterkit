//! Hello World notification demo.
use waterkit_notification::Notification;

fn main() {
    println!("Sending notification...");
    Notification::new()
        .title("Hello")
        .body("World from WaterKit!")
        .show();
    println!("Notification sent.");
}
