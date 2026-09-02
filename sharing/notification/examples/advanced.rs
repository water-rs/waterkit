//! Advanced notification example showing all features.

use std::path::PathBuf;
use waterkit_notification::{Icon, InterruptionLevel, Notification, Sound, Timeout};

fn main() -> Result<(), waterkit_notification::NotificationError> {
    // Basic notification with all options
    Notification::new()
        .title("Download Complete")
        .body("Your file has been downloaded successfully.")
        .subtitle("Files App") // macOS/iOS only
        .app_name("WaterKit Demo")
        .icon(Icon::Theme("folder-download".into())) // Linux theme icon
        .sound(Sound::Theme("complete".into())) // Linux sound theme
        .interruption_level(InterruptionLevel::Active)
        .timeout(Timeout::Milliseconds(5000)) // Linux only
        .show()?;

    println!("Notification sent!");

    // Time-sensitive notification (breaks through Focus/DND)
    Notification::new()
        .title("Urgent: Meeting Starting")
        .body("Your meeting with the team starts in 5 minutes")
        .interruption_level(waterkit_notification::InterruptionLevel::TimeSensitive)
        .show()?;

    println!("Time-sensitive notification sent!");

    // Notification with file-based icon
    let icon_path = PathBuf::from("/usr/share/icons/hicolor/48x48/apps/firefox.png");
    if icon_path.exists() {
        Notification::new()
            .title("Custom Icon")
            .body("This notification uses a file-based icon.")
            .icon(Icon::File(icon_path))
            .show()?;

        println!("Custom icon notification sent!");
    }

    // Silent notification (passive)
    Notification::new()
        .title("Background Task")
        .body("Sync completed in the background.")
        .interruption_level(InterruptionLevel::Passive)
        .sound(Sound::Suppress)
        .show()?;

    println!("Silent notification sent!");

    Ok(())
}
