//! Basic notification example.

use waterkit_notification::Notification;

fn main() -> Result<(), waterkit_notification::NotificationError> {
    pollster::block_on(run())
}

async fn run() -> Result<(), waterkit_notification::NotificationError> {
    // Toasts are attributed to an AppUserModelID; unpackaged apps register one
    // before the first notification. Keep the registration alive for the
    // lifetime of the process's notifications.
    #[cfg(target_os = "windows")]
    let _registration = waterkit_notification::windows::AppIdentity::new(
        "dev.waterkit.examples.hello-world",
        "WaterKit Hello World",
    )
    .register()?;

    println!("Sending notification...");

    Notification::new()
        .title("Hello")
        .body("World from WaterKit!")
        .show()
        .await?;

    println!("Notification sent.");
    Ok(())
}
