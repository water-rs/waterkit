# Waterkit Screen

GPU-first zero-copy screen capture with wgpu texture output.

## Features

- **Simple Screenshot**: Device-free capture with PNG/HEIF/AVIF encoding
- **GPU Streaming**: Zero-copy screen capture with wgpu texture output
- **Screen Info**: Resolution, scaling factor, and display enumeration
- **Brightness Control**: Get/set screen brightness (platform-dependent)

## Installation

```toml
[dependencies]
waterkit-screen = "0.1"
```

## Platform Support

| Platform | Screenshot | GPU Streaming | Brightness |
| :--- | :---: | :---: | :---: |
| **macOS** | PNG/HEIF/AVIF | IOSurface → wgpu | Stub |
| **Windows** | PNG | wgpu upload | Stub |
| **Linux** | PNG | wgpu upload | Stub |
| **iOS** | PNG/HEIF/AVIF | - | UIKit |
| **Android** | - | - | Settings API |

## Usage

### Simple Screenshot

```rust
use waterkit_screen::{screenshot_primary, ImageFormat};

fn main() {
    // Capture as PNG (no GPU device needed)
    let shot = screenshot_primary(ImageFormat::Png).unwrap();
    shot.save("screenshot.png").unwrap();

    // Capture as HEIF (macOS/iOS only)
    let shot = screenshot_primary(ImageFormat::Heif).unwrap();
    shot.save("screenshot.heic").unwrap();
}
```

### GPU Streaming

```rust
use waterkit_screen::{screens, ScreenStream, StreamConfig};
use std::sync::Arc;

fn main() {
    // Initialize wgpu (caller-provided)
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let device = Arc::new(device);
    let queue = Arc::new(queue);

    // Get primary display
    let displays = screens().unwrap();
    let primary = displays.iter().find(|d| d.is_primary()).unwrap();

    // Start capture stream
    let config = StreamConfig { target_fps: 60, show_cursor: true };
    let stream = ScreenStream::start(primary, device, queue, &config).unwrap();

    // Capture frames as GPU textures
    while let Some(frame) = stream.try_next_frame() {
        let view = frame.create_view();
        // Use view in render pipeline...
    }
}
```

## Examples

```bash
# Simple screenshot
cargo run --example screenshot

# GPU streaming statistics
cargo run --example stream_preview

# Basic demo
cargo run --example demo
```
