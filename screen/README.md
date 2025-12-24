# Waterkit Screen

Screen capture and display information.

## Features

- **Screen Info**: Resolution, Scaling Factor, name of connected displays.
- **Capture**: Screenshot current screen.
- **Recording**: (Beta) Record screen to file.

## Installation

```toml
[dependencies]
waterkit-screen = "0.1"
```

## Platform Support

| Platform | Backend |
| :--- | :--- |
| **macOS** | `ScreenCaptureKit` (12.3+) / `CGWindowList` |
| **Windows/Linux** | `arboard` (Screenshots), `scrap` (Capture) |
| **Android/iOS** | *Limited Support* (Screenshot often restricted by OS) |

## Usage

```rust
use waterkit_screen::ScreenCapturer;

async fn take_screenshot() {
    let capturer = ScreenCapturer::new().await.unwrap();
    let image = capturer.capture_primary_display().await.unwrap();
    
    // image is dynamic generic image buffer
}
```
