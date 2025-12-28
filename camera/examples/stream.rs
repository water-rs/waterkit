//! Camera streaming example.
//!
//! Lists available cameras, opens the default one, and streams frames.

use futures::StreamExt;
use std::pin::pin;
use std::sync::Arc;
use waterkit_camera::{Camera, CameraError};

#[tokio::main]
async fn main() -> Result<(), CameraError> {
    // List available cameras
    println!("Available cameras:");
    let cameras = Camera::list()?;
    for camera in &cameras {
        println!(
            "  - {} (id: {}, front: {})",
            camera.name, camera.id, camera.is_front_facing
        );
    }

    if cameras.is_empty() {
        println!("No cameras found!");
        return Ok(());
    }

    // Create wgpu instance and device
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .expect("Failed to find adapter");

    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .expect("Failed to create device");

    let device = Arc::new(device);
    let queue = Arc::new(queue);

    // Open the default camera (starts streaming immediately)
    println!("\nOpening default camera...");
    let camera = Camera::open_default(device.clone(), queue.clone()).await?;

    println!(
        "Camera opened: {}x{}",
        camera.resolution().width,
        camera.resolution().height
    );

    // Print capabilities
    let caps = camera.capabilities();
    println!("\nCapabilities:");
    println!("  Resolutions: {:?}", caps.resolutions);
    println!("  Frame rates: {:?}", caps.frame_rates);
    println!("  ISO range: {:?}", caps.iso_range);
    println!("  Manual focus: {}", caps.supports_manual_focus);
    println!("  HDR: {}", caps.supports_hdr);
    println!("  Flash: {}", caps.has_flash);

    // Stream some frames
    println!("\nStreaming frames (will stop after 10 frames)...");
    let mut frame_count = 0;

    {
        let mut frames = pin!(camera.frames());
        while let Some(frame) = frames.next().await {
            frame_count += 1;
            println!(
                "Frame {}: {}x{} {:?} @ {:?}",
                frame_count,
                frame.width(),
                frame.height(),
                frame.format(),
                frame.timestamp()
            );

            // Stop after 10 frames for demo
            if frame_count >= 10 {
                break;
            }
        }
    }

    println!("\nReceived {} frames", frame_count);

    // Camera stops when dropped
    println!("Camera closed");

    Ok(())
}
