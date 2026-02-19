# Waterkit Camera

Cross-platform camera streaming, controls, photo capture, recording, and RAW workflows.

## Installation

```toml
[dependencies]
waterkit-camera = "0.1"
# or
waterkit = { version = "0.1", features = ["camera"] }
```

## Modern Feature Surface

`Camera::capabilities()` exposes platform-verified support for:

- Dynamic range profiles (`SDR`, `HDR10`, `HLG10`, `DolbyVision` when available).
- Dolby Vision availability (`supports_dolby_vision`).
- Multi-camera concurrency (`supports_concurrent_multi_camera`, `max_concurrent_cameras`).
- System-native photo/video pipelines (`uses_system_photo_pipeline`, `uses_system_video_pipeline`).
- RAW photo and RAW video support + formats.

These fields are validated internally to fail fast on inconsistent backend reports.

## Core APIs

- Camera discovery: `Camera::list()`
- Open camera: `Camera::open(...)`, `Camera::open_default(...)`
- GPU frame stream: `Camera::frames()`
- Controls: `Camera::apply_controls(...)`
- Photo capture: `Camera::capture_photo()`
- RAW photo capture: `Camera::capture_raw_photo()`
- Video recording: `Camera::recording(path)`
- RAW video recording: `Camera::raw_recording(path)`

## RAW Outputs

- RAW photo: DNG payload via `RawPhoto`.
- RAW video: uncompressed frame stream file (`WKRV` container):
  - Header: magic/version/pixel-format/width/height/fps
  - Per frame: `timestamp_ns(u64 LE) + payload_len(u32 LE) + raw pixels`
  - Android: `RGBA8` frames
  - Apple: `BGRA8` frames

## Example: Capability Probe

```rust
use std::sync::Arc;
use waterkit_camera::{Camera, CameraError};

#[tokio::main]
async fn main() -> Result<(), CameraError> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .expect("adapter");
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .expect("device");

    let mut camera = Camera::open_default(Arc::new(device), Arc::new(queue)).await?;
    let caps = camera.capabilities();

    tracing::info!("dynamic_ranges={:?}", caps.dynamic_ranges);
    tracing::info!("dolby_vision={}", caps.supports_dolby_vision);
    tracing::info!(
        "multi_camera={} max={}",
        caps.supports_concurrent_multi_camera, caps.max_concurrent_cameras
    );
    tracing::info!(
        "system_pipeline photo={} video={}",
        caps.uses_system_photo_pipeline, caps.uses_system_video_pipeline
    );
    tracing::info!(
        "raw_photo={} formats={:?}",
        caps.supports_raw_photo, caps.raw_photo_formats
    );
    tracing::info!(
        "raw_video={} formats={:?}",
        caps.supports_raw_video, caps.raw_video_formats
    );

    if caps.supports_raw_photo {
        let raw = camera.capture_raw_photo().await?;
        tracing::info!(
            "captured RAW photo: {} bytes, {}x{}, {:?}",
            raw.data().len(),
            raw.width(),
            raw.height(),
            raw.format()
        );
    }

    Ok(())
}
```

## Platform Backends

| Platform | Backend |
| :--- | :--- |
| iOS / macOS | AVFoundation + Swift bridge |
| Android | Camera2 + MediaRecorder + Kotlin bridge |
| Windows / Linux | `nokhwa` |

## Permissions

- iOS: add `NSCameraUsageDescription`.
- Android: add `<uses-permission android:name="android.permission.CAMERA" />`.
