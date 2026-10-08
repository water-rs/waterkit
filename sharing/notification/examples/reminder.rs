//! A reminder toast that persists until dismissed.
//!
//! `Timeout::Never` maps to the `WinRT` reminder scenario, which requires at
//! least one action button.

use waterkit_notification::{Action, Notification, Timeout};

fn main() -> Result<(), waterkit_notification::NotificationError> {
    pollster::block_on(run())
}

async fn run() -> Result<(), waterkit_notification::NotificationError> {
    println!("Showing a reminder toast; it stays on screen until dismissed...");

    Notification::new()
        .title("Backup Complete")
        .body("42 GB uploaded to cloud storage")
        .timeout(Timeout::Never)
        .action(Action::new("View Backup", "https://waterui.dev"))
        .show()
        .await?;

    println!("Notification sent!");

    std::thread::sleep(std::time::Duration::from_secs(5));

    Ok(())
}
