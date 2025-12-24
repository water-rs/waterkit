# Waterkit Notification

Local system notifications.

## Features

- **Local Alerts**: Schedule notifications with title and body.
- **Scheduling**: Immediate or delayed delivery.
- **Sound**: Default system notification sound.

## Installation

```toml
[dependencies]
waterkit-notification = "0.1"
```

## Platform Support

| Platform | Backend |
| :--- | :--- |
| **macOS** | `NSUserNotificationCenter` / `UNUserNotificationCenter` |
| **iOS** | `UNUserNotificationCenter` |
| **Android** | `NotificationManager` |
| **Linux/Windows** | `notify-rust` |

## Usage

```rust
use waterkit_notification::Notification;

async fn notify() {
    Notification::new()
        .title("Task Complete")
        .body("Your file has been downloaded.")
        .show()
        .await
        .unwrap();
}
```
