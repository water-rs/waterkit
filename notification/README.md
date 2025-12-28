# Waterkit Notification

Cross-platform local notifications for Rust.

## Platform Support

| Feature | Linux | macOS | Windows | iOS | Android |
|---------|:-----:|:-----:|:-------:|:---:|:-------:|
| title/body | ✓ | ✓ | ✓ | ✓ | ✓ |
| icon | ✓ | ✓ | ✓ | ✗ | ✗ |
| subtitle | ✗ | ✓ | ✗ | ✓ | ✗ |
| interruption_level | ✓ | ✗ | ✗ | ✓ | ✓ |
| timeout | ✓ | ✗ | ✗ | ✗ | ✗ |
| sound | ✓ | ✗ | ✗ | ✓ | ✓ |
| actions (URL) | ✓ | ✓ | ✗ | ✓ | ✓ |

## Installation

```toml
[dependencies]
waterkit-notification = "0.1"
```

## Quick Start

```rust
use waterkit_notification::Notification;

fn main() -> Result<(), waterkit_notification::NotificationError> {
    Notification::new()
        .title("Hello")
        .body("World from WaterKit!")
        .show()?;
    Ok(())
}
```

## Time-Sensitive Notifications

Break through Focus/DND modes with time-sensitive notifications:

```rust
use waterkit_notification::Notification;

Notification::new()
    .title("Meeting Starting")
    .body("Your meeting starts in 5 minutes")
    .time_sensitive()
    .show()?;
```

### Interruption Levels

| Level | iOS | Android | Linux |
|-------|-----|---------|-------|
| `Passive` | Silent | `IMPORTANCE_LOW` | Low urgency |
| `Active` (default) | Standard | `IMPORTANCE_DEFAULT` | Normal urgency |
| `TimeSensitive` | Breaks Focus | `IMPORTANCE_HIGH` | Critical urgency |
| `Critical` | Breaks silent* | `IMPORTANCE_MAX` | Critical urgency |

*iOS Critical requires Apple entitlement approval.

## Action Buttons

Add buttons that open URLs or deep links when clicked:

```rust
use waterkit_notification::{Notification, Action};

Notification::new()
    .title("WaterUI Update Available")
    .body("A new version of WaterUI is ready to download")
    .action(Action::new("View Details", "https://waterui.dev"))
    .action(Action::new("Later", "waterui://dismiss"))
    .show()?;
```

Actions work on Linux, macOS, iOS, and Android. Each action opens the specified URL when clicked.

**macOS Note:** Actions require a bundled `.app` application. CLI tools will receive a clear error if actions are requested.

## Advanced Features

```rust
use waterkit_notification::{Notification, Icon, Sound, Timeout, InterruptionLevel};

Notification::new()
    .title("Download Complete")
    .body("Your file has been downloaded.")
    .subtitle("Files App")                    // macOS/iOS only
    .app_name("My App")
    .icon(Icon::Theme("folder-download".into())) // Linux theme icon
    .sound(Sound::Theme("complete".into()))   // Linux sound theme
    .timeout(Timeout::Milliseconds(5000))     // Linux only
    .interruption_level(InterruptionLevel::Active)
    .show()?;
```

## Platform Backends

| Platform | Backend |
|----------|---------|
| Linux | D-Bus (freedesktop.org notifications) |
| macOS | `UserNotifications` (bundled apps) / `notify-rust` (CLI) |
| Windows | `notify-rust` (winrt-notification) |
| iOS | `UserNotifications` framework |
| Android | `NotificationManager` with channels |

## Android Notes

On Android, you must use `show_with_context()` with a valid Android `Context`:

```rust
#[cfg(target_os = "android")]
notification.show_with_context(&mut env, &context)?;
```
