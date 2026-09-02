//! GPU screen capture streaming example.
//!
//! Demonstrates capturing screen frames as GPU textures using `ScreenStream`.
//! This example captures frames and prints statistics without rendering.
//!
//! Run: `cargo run --example stream_preview`

use std::sync::Arc;
use std::time::{Duration, Instant};
use waterkit_screen::{ScreenStream, StreamConfig, screens};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("WaterKit Screen - GPU Streaming Example");

    // Initialize wgpu
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))?;

    println!("Using adapter: {:?}", adapter.get_info().name);

    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("ScreenCapture"),
        ..Default::default()
    }))?;

    let device: Arc<wgpu::Device> = Arc::new(device);
    let queue: Arc<wgpu::Queue> = Arc::new(queue);

    // Get primary display
    let displays = screens()?;
    let primary = displays
        .iter()
        .find(|d| d.is_primary())
        .or_else(|| displays.first())
        .ok_or("No display found")?;

    println!(
        "Capturing from: {} ({}x{})",
        primary.name(),
        primary.width(),
        primary.height()
    );

    // Start capture stream
    let config = StreamConfig {
        target_fps: 60,
        show_cursor: true,
    };

    let stream = ScreenStream::start(primary, device, queue, &config)?;

    println!("Stream started. Capturing for 5 seconds...\n");

    let start = Instant::now();
    let duration = Duration::from_secs(5);
    let mut frame_count = 0u64;
    let mut last_report = Instant::now();

    while start.elapsed() < duration {
        if let Some(frame) = stream.try_next_frame() {
            frame_count += 1;

            // Report every second
            if last_report.elapsed() >= Duration::from_secs(1) {
                #[allow(clippy::cast_precision_loss)]
                let fps = frame_count as f64 / start.elapsed().as_secs_f64();
                println!(
                    "Frame {frame_count}: {}x{} {:?} @ {fps:.1} FPS",
                    frame.width(),
                    frame.height(),
                    frame.format(),
                );
                last_report = Instant::now();
            }
        }

        // Small sleep to avoid busy-waiting
        std::thread::sleep(Duration::from_micros(100));
    }

    let elapsed = start.elapsed().as_secs_f64();
    #[allow(clippy::cast_precision_loss)]
    let fps = frame_count as f64 / elapsed;

    println!("\n--- Statistics ---");
    println!("Duration: {elapsed:.2}s");
    println!("Frames captured: {frame_count}");
    println!("Average FPS: {fps:.1}");
    println!("Texture format: {:?}", wgpu::TextureFormat::Bgra8UnormSrgb);

    Ok(())
}
