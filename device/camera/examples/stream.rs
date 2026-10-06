//! Camera streaming example.
//!
//! Lists available cameras, opens the default one, streams a few frames and
//! converts each to an upright RGBA texture on the GPU. Run with
//! `RUST_LOG=info` to see the output.

use futures::StreamExt;
use std::pin::pin;
use std::sync::Arc;
use waterkit_camera::{Camera, CameraError, FrameConverter, FramePlanes};

fn main() -> Result<(), CameraError> {
    tracing_subscriber::fmt::init();
    pollster::block_on(stream())
}

#[cfg_attr(
    target_arch = "wasm32",
    expect(
        clippy::future_not_send,
        reason = "on wasm32 `wgpu::Device` and `wgpu::Queue` are not `Send`, so neither is a future holding them"
    )
)]
async fn stream() -> Result<(), CameraError> {
    let cameras = Camera::list()?;
    for camera in &cameras {
        tracing::info!(
            "camera {} (id: {}, front: {})",
            camera.name,
            camera.id,
            camera.is_front_facing
        );
    }
    if cameras.is_empty() {
        tracing::warn!("no cameras found");
        return Ok(());
    }

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .expect("Failed to find adapter");
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            required_features: FrameConverter::required_features(adapter.features()),
            ..Default::default()
        })
        .await
        .expect("Failed to create device");

    #[cfg_attr(
        target_arch = "wasm32",
        expect(
            clippy::arc_with_non_send_sync,
            reason = "`Camera::open_default` takes the device in an `Arc` on every platform; on wasm32 `wgpu::Device` is neither `Send` nor `Sync`"
        )
    )]
    let device = Arc::new(device);
    let queue = Arc::new(queue);

    // The camera starts streaming when it opens.
    let camera = Camera::open_default(device.clone(), queue.clone()).await?;
    let resolution = camera.resolution();
    tracing::info!("camera opened: {}x{}", resolution.width, resolution.height);

    let caps = camera.capabilities();
    tracing::info!("resolutions: {:?}", caps.resolutions);
    tracing::info!("frame rates: {:?}", caps.frame_rates);
    tracing::info!("ISO range: {:?}", caps.iso_range);
    tracing::info!("manual focus: {}", caps.supports_manual_focus);
    tracing::info!("dynamic ranges: {:?}", caps.dynamic_ranges);
    tracing::info!("Dolby Vision: {}", caps.supports_dolby_vision);
    tracing::info!("flash: {}", caps.has_flash);

    let mut converter = FrameConverter::new(&device);
    let mut output = None;
    let mut frame_count = 0;
    {
        let mut frames = pin!(camera.frames());
        while let Some(frame) = frames.next().await {
            let frame = frame?;
            frame_count += 1;
            let layout = match frame.planes() {
                FramePlanes::Rgb(_) => "RGB",
                FramePlanes::YCbCr420 { .. } => "YCbCr 4:2:0",
                FramePlanes::YCbCr422 { .. } => "YCbCr 4:2:2",
            };
            // One output texture serves every frame of the same upright size.
            let upright = output
                .take()
                .filter(|texture: &wgpu::Texture| {
                    texture.size() == FrameConverter::upright_size(&frame)
                })
                .unwrap_or_else(|| FrameConverter::create_output(&device, &frame));
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("camera frame"),
            });
            converter.encode(&device, &mut encoder, &frame, &upright);
            queue.submit([encoder.finish()]);
            tracing::info!(
                "frame {frame_count}: {}x{} {layout} {:?}, upright {}x{} @ {:?}",
                frame.width(),
                frame.height(),
                frame.orientation(),
                upright.width(),
                upright.height(),
                frame.timestamp()
            );
            output = Some(upright);

            if frame_count >= 10 {
                break;
            }
        }
    }

    tracing::info!("received {frame_count} frames; the camera closes when dropped");
    Ok(())
}
