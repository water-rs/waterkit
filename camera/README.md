# Waterkit Camera

Cross-platform camera access and streaming library.

## Features

- **Device Enumeration**: List available cameras (front, back, external).
- **Preview Stream**: Get raw frame data for rendering (compatible with `wgpu`).
- **Capture**: Take high-quality photos.
- **Controls**: (Roadmap) Focus, Zoom, Flash.

## Installation

```toml
[dependencies]
waterkit-camera = "0.1"
# OR
waterkit = { version = "0.1", features = ["camera"] }
```

## Platform Support

| Platform | Backend |
| :--- | :--- |
| **macOS/iOS** | AVFoundation (Native Swift) |
| **Android** | Camera2 API (Native Kotlin) |
| **Windows/Linux** | `nokhwa` (Rust) |

## Usage

```rust
use waterkit_camera::{CameraManager, CameraPosition};

async fn start_camera() {
    let manager = CameraManager::new().await.unwrap();
    
    // Get list of cameras
    let cameras = manager.get_devices().await.unwrap();
    
    // Select the back camera
    if let Some(back_cam) = cameras.iter().find(|c| c.position == CameraPosition::Back) {
        let stream = manager.start_stream(back_cam).await.unwrap();
        
        // Use stream with wgpu or other renderer
        // stream.get_frame()...
    }
}
```

## Permissions

**iOS**: Add `NSCameraUsageDescription`.
**Android**: Add `<uses-permission android:name="android.permission.CAMERA" />`.
