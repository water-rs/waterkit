# Waterkit Haptic

Cross-platform haptic feedback and vibration control.

## Features

- **Impact feedback** with customizable intensity (0.0-1.0)
- **Notification feedback** for success, warning, and error states
- **Selection feedback** for UI selection changes
- **Custom patterns** via builder API for complex vibration sequences

## Installation

```toml
[dependencies]
waterkit-haptic = "0.1"
```

## Platform Support

| Platform | Backend | Features |
| :--- | :--- | :--- |
| **iOS** | `UIImpactFeedbackGenerator`, Core Haptics | Full support including custom patterns |
| **macOS** | `NSHapticFeedbackManager` | Basic feedback only |
| **Android** | `VibrationEffect` | Full support with amplitude control |
| **Windows** | `SimpleHapticsController` | Basic feedback with intensity |
| **Linux** | `feedbackd` (`org.sigxcpu.Feedback`) | Event-based haptic feedback and patterns on environments exposing feedbackd |

## Usage

```rust
use waterkit_haptic::{Haptic, HapticPattern, Intensity};
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Check availability
    if !Haptic::is_available() {
        return Ok(());
    }

    // Simple impact feedback
    Haptic::impact(Intensity::MEDIUM)?;

    // Use predefined intensity levels
    Haptic::impact(Intensity::LOW)?;
    Haptic::impact(Intensity::HIGH)?;
    Haptic::impact(Intensity::MAX)?;

    // Or custom validated intensity (0.0-1.0)
    Haptic::impact(Intensity::new(0.7)?)?;

    // Selection feedback (light tap)
    Haptic::selection()?;

    // Notification feedback
    Haptic::notification_success()?;
    Haptic::notification_warning()?;
    Haptic::notification_error()?;

    // Custom pattern
    let pattern = HapticPattern::builder()
        .add(Duration::from_millis(100), Intensity::MAX)
        .pause(Duration::from_millis(50))
        .add(Duration::from_millis(200), Intensity::MEDIUM)
        .pause(Duration::from_millis(50))
        .add(Duration::from_millis(100), Intensity::LOW)
        .build();

    Haptic::play(&pattern)?;

    Ok(())
}
```

## Intensity Levels

| Constant | Value | Use Case |
| :--- | :--- | :--- |
| `Intensity::LOW` | 0.25 | Subtle feedback |
| `Intensity::MEDIUM` | 0.5 | Default feedback |
| `Intensity::HIGH` | 0.75 | Strong feedback |
| `Intensity::MAX` | 1.0 | Maximum intensity |
