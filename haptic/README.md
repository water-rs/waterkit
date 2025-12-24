# Waterkit Haptic

Haptic feedback and vibration control.

## Features

- **Impact**: Light, Medium, Heavy impact styles.
- **Notification**: Success, Warning, Error feedback patterns.
- **Selection**: Subtle tick for UI selection changes.

## Installation

```toml
[dependencies]
waterkit-haptic = "0.1"
```

## Platform Support

| Platform | Backend |
| :--- | :--- |
| **iOS** | `UIImpactFeedbackGenerator`, `UINotificationFeedbackGenerator` |
| **Android** | `Vibrator` / `HapticFeedbackConstants` |
| **Desktop** | *No-op (ignored safe)* |

## Usage

```rust
use waterkit_haptic::{HapticFeedback, ImpactStyle, NotificationType};

async fn feedback() {
    let haptics = HapticFeedback::new().await.unwrap();
    
    // UI Selection tick
    haptics.selection_changed().await;
    
    // Heavy impact
    haptics.impact(ImpactStyle::Heavy).await;
    
    // Success notification
    haptics.notification(NotificationType::Success).await;
}
```
