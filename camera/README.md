# waterkit-camera

Cross-platform camera streaming for WaterUI with efficient WGPU texture integration.

## Features

- **Camera Enumeration**: List available cameras with metadata
- **Frame Capture**: Stream frames from camera devices
- **WGPU Integration**: Efficient frame-to-texture conversion
- **Cross-Platform**: iOS, macOS, Android, Windows, Linux

## Usage

```rust
use waterkit_camera::{Camera, CameraFrame};

// List cameras
let cameras = Camera::list()?;

// Open default camera
let mut camera = Camera::open_default()?;

// Start capturing
camera.start()?;

// Get a frame
let frame: CameraFrame = camera.get_frame()?;

// Write to WGPU texture
frame.write_to_texture(&queue, &texture);
```

## Platform Backends

| Platform | Backend |
|----------|---------|
| macOS | AVCaptureSession via swift-bridge |
| iOS | AVCaptureSession via swift-bridge |
| Windows | nokhwa (MSMF) |
| Linux | nokhwa (V4L2) |
| Android | Camera2 API via JNI |

## License

MIT
