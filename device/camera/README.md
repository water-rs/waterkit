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
- Upright RGBA on the GPU: `FrameConverter`
- Controls: `Camera::apply_controls(...)`
- Photo capture: `Camera::capture_photo()`
- RAW photo capture: `Camera::capture_raw_photo()`
- Video recording: `Camera::recording(path)`
- RAW video recording: `Camera::raw_recording(path)`

## Frames

A `Frame` exposes what it holds rather than a hidden RGBA texture:

- `Frame::planes()` returns `FramePlanes`: one 8-bit RGBA/BGRA texture
  (`Rgb`), biplanar 4:2:0 YCbCr (`YCbCr420`, luma plus interleaved chroma) or
  packed 4:2:2 YUYV (`YCbCr422`), with the `YcbcrEncoding` (matrix and range)
  of the YCbCr layouts.
- `Frame::orientation()` says how the stored pixels relate to upright, with
  EXIF 1–8 semantics. Upright is the scene as the lens sees it, unmirrored.
- `FrameConverter` renders any frame to an upright `Rgba8Unorm` texture in a
  compute pass. Its shaders are compiled ahead of time with `shaderloom`, so
  create the device with `FrameConverter::required_features(adapter.features())`.
  Windows and Linux convert photos with it, so there a camera opens only on
  such a device and `Camera::open` returns `CameraError::GpuError` otherwise.
- `Camera::frames()` yields `Result<Frame, CameraError>`: a capture or import
  failure arrives as the stream's last item.

Frame timestamps: `Frame::timestamp()` reads zero on the first frame the
camera delivered after it opened and measures every later frame on the platform's
capture clock — the sample buffer's presentation time on Apple, the image's
sensor timestamp (start of exposure) on Android, and the capture thread's
receipt time on Windows and Linux. Timestamps are monotonic within one open
but are not wall-clock time and are not comparable between cameras or
between two opens of the same camera.

What each platform delivers today:

| Platform | Planes | Orientation |
| :--- | :--- | :--- |
| iOS / macOS | `YCbCr420`: the capture buffer's `IOSurface` planes (`420f`, else `420v`), no copy | upright on the display: the foreground scene's interface orientation on iOS, the rotation coordinator's horizon-level angle on macOS; plus the connection's mirroring |
| Android | `YCbCr420`: the camera's GPU-sampled `AHardwareBuffer`, imported (driver-private formats are converted into plane textures on the GPU) | sensor orientation, lens facing, display rotation |
| Windows / Linux | `YCbCr420` (NV12), `YCbCr422` (YUYV), or `Rgb` from MJPEG | always `Up` |

On Apple and Android no pixel of a frame passes through the CPU, and a
frame's textures alias a buffer from the camera's small pool. On Apple the
capture buffer returns to the pool when the frame drops and the GPU work
submitted until then finishes; on Android the camera's `AHardwareBuffer` is
imported through `wgpu-external-frame`, which returns it to the reader once
the frame drops and the GPU no longer reads it. On both, a consumer that holds
frames empties the pool and the camera drops new frames until one comes back —
on Android at most two frames may be held at once — so drop each frame as soon
as its work is submitted.

On Android, open the device with
`wgpu_external_frame::ahardware_buffer::request_device` (re-exported as
`waterkit_camera::wgpu_external_frame`) and request
`wgpu::Features::TEXTURE_FORMAT_NV12`; `Camera::open` returns
`CameraError::GpuError` on a device without them.

Desktop uploads the planes the webcam delivers into textures created for each
frame.

## Analysis Frames

`CameraConfig::analysis` opts into a second stream of CPU-readable
`Y'CbCr` 4:2:0 frames for image analysis — barcode scanning, text
recognition, and friends — next to the GPU frame stream:

```rust
use futures::StreamExt;
use waterkit_camera::{AnalysisConfig, Camera, CameraConfig};

let config = CameraConfig {
    analysis: Some(AnalysisConfig::default()), // 1280×720
    ..CameraConfig::default()
};
let camera = Camera::open(&camera_id, config, device, queue).await?;
let mut analysis = camera.analysis_frames();
while let Some(frame) = analysis.next().await {
    let frame = frame?;
    let planes = frame.planes(); // luma, Cb and Cr slices with strides
    // or hand it to waterkit-vision: `Image::from(&frame)`
}
```

- `AnalysisFrame` carries width/height, `Orientation`, `VideoColorInfo`, and
  a timestamp on the same capture clock `Frame::timestamp` reads.
- The stream is newest-wins: a consumer that falls behind drops pending
  frames rather than stalling the camera.
- Calling `analysis_frames` on a camera opened without `analysis` yields
  `CameraError::AnalysisNotConfigured` once and ends.
- Platform payloads: on Android the frame holds the acquired
  `android.media.Image` (`AnalysisFrame::media_image`), closed when the last
  clone drops; on iOS/macOS it retains the capture output's `CVPixelBuffer`
  (`AnalysisFrame::pixel_buffer`), which the CPU reads without a copy; on
  Windows/Linux it holds the pixels the capture thread delivered, before
  upload.

## RAW Outputs

- RAW photo: DNG payload via `RawPhoto`.
- RAW video: uncompressed frame stream file (`WKRV` version 2):
  - The 20-byte little-endian header contains magic `WKRV`, version `2`,
    pixel format (`2` = RGBA8, `3` = NV12), H.273 matrix code, range (`0` =
    limited, `1` = full), then width, height, and fps as `u32`.
  - RGBA8 uses identity matrix code `0` and full range. NV12 records matrix
    code `1` (BT.709), `6` (BT.601), or `9` (BT.2020 non-constant-luminance)
    and its captured range. NV12 payloads contain luma rows followed by
    interleaved CbCr rows, without padding.
  - Each frame is `timestamp_ns(u64 LE) + payload_len(u32 LE) + pixels`.
  - Version 1 is rejected: it does not record its YCbCr matrix, so its frames
    cannot be decoded unambiguously. Re-record with version 2.
  - `RawVideoReader` reads and validates the header and frames. Its
    `RawVideoFrame::to_rgba` converts NV12 using the recorded matrix and range;
    output is non-linear R′G′B′, with no primaries or transfer conversion
    because those values are not stored in WKRV.
  - Desktop recordings store RGBA8; Apple and Android store NV12 with matrix
    and range metadata from the first captured frame.

## Example: Capability Probe

```rust
use std::sync::Arc;
use waterkit_camera::{Camera, CameraError, FrameConverter, wgpu};

#[tokio::main]
async fn main() -> Result<(), CameraError> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .expect("adapter");
    // Windows and Linux convert photos with `FrameConverter`.
    let required_features = FrameConverter::required_features(adapter.features());
    // Android imports camera buffers, which needs the import's device
    // extensions and NV12 textures.
    #[cfg(target_os = "android")]
    let (device, queue) = waterkit_camera::wgpu_external_frame::ahardware_buffer::request_device(
        &adapter,
        &wgpu::DeviceDescriptor {
            required_features: required_features | wgpu::Features::TEXTURE_FORMAT_NV12,
            ..Default::default()
        },
    )
    .expect("device");
    #[cfg(not(target_os = "android"))]
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            required_features,
            ..Default::default()
        })
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
| iOS / macOS | AVFoundation + objc2 |
| Android | Camera2 + MediaRecorder + Kotlin bridge |
| Windows / Linux | `nokhwa` |

## Permissions

- iOS: add `NSCameraUsageDescription`.
- Android: add `<uses-permission android:name="android.permission.CAMERA" />`.