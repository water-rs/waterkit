//! Quick profiling test for screen capture performance.
//!
//! Tests screenshot latency and GPU streaming capture throughput.

use std::sync::Arc;
use std::time::{Duration, Instant};
use waterkit_codec::{CodecType, Encoder};
use waterkit_screen::{ImageFormat, ScreenStream, StreamConfig, screens, screenshot};

const ITERATIONS: usize = 100;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    println!(
        "Screen Capture Performance Test ({} iterations each)\n",
        ITERATIONS
    );

    // Get screen info
    let displays = screens()?;
    let primary = displays
        .iter()
        .find(|d| d.is_primary())
        .unwrap_or(&displays[0]);
    println!(
        "Screen: {} ({}x{})\n",
        primary.name(),
        primary.width(),
        primary.height()
    );

    // Test 1: Screenshot capture (PNG encoding)
    println!("=== Test 1: Screenshot Capture (PNG) ===");
    {
        let start = Instant::now();
        for _ in 0..ITERATIONS {
            let _ = screenshot(primary, ImageFormat::Png)?;
        }
        let total = start.elapsed();
        println!(
            "Total: {:?}, Avg: {:?}/frame, FPS: {:.1}\n",
            total,
            total / ITERATIONS as u32,
            ITERATIONS as f64 / total.as_secs_f64()
        );
    }

    // Test 2: GPU Streaming Capture
    println!("=== Test 2: GPU Streaming Capture ===");
    {
        // Create wgpu device
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
        let device: Arc<wgpu::Device> = Arc::new(device);
        let queue: Arc<wgpu::Queue> = Arc::new(queue);

        let config = StreamConfig {
            target_fps: 120,
            show_cursor: false,
        };

        let stream = ScreenStream::start(primary, device.clone(), queue.clone(), &config)?;

        // Wait for stream to warm up
        std::thread::sleep(Duration::from_millis(500));

        println!("Running 5-second capture test...");
        let duration = Duration::from_secs(5);
        let start = Instant::now();
        let mut frame_count = 0u64;

        while start.elapsed() < duration {
            if stream.try_next_frame().is_some() {
                frame_count += 1;
            }
        }

        let total = start.elapsed();
        let fps = frame_count as f64 / total.as_secs_f64();
        println!("Duration: {:?}", total);
        println!("Frames captured: {}", frame_count);
        println!("**GPU Streaming FPS: {:.1}**\n", fps);
    }

    // Test 3: GPU Streaming + H.265 Encode
    println!("=== Test 3: GPU Streaming + H.265 Encode ===");
    {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
        let device: Arc<wgpu::Device> = Arc::new(device);
        let queue: Arc<wgpu::Queue> = Arc::new(queue);

        let config = StreamConfig {
            target_fps: 60,
            show_cursor: false,
        };

        let stream = ScreenStream::start(primary, device.clone(), queue.clone(), &config)?;
        let (width, height) = stream.dimensions();

        let mut encoder = Encoder::new(CodecType::H265, width, height)?;

        // Wait for stream to warm up
        std::thread::sleep(Duration::from_millis(500));

        let start = Instant::now();
        let mut capture_time = Duration::ZERO;
        let mut encode_time = Duration::ZERO;
        let mut frame_count = 0usize;

        for _ in 0..50 {
            let t = Instant::now();
            if let Some(frame) = stream.try_next_frame() {
                capture_time += t.elapsed();

                // For encoding, we need NV12 data. The frame is a GPU texture.
                // In a real pipeline, we'd use IOSurface encoding or read back the texture.
                // For this benchmark, we'll create dummy NV12 data.
                let y_size = (frame.width() * frame.height()) as usize;
                let nv12_data = vec![128u8; y_size + y_size / 2];

                let t = Instant::now();
                for result in encoder.encode_nv12(&nv12_data) {
                    let _ = result;
                }
                encode_time += t.elapsed();
                frame_count += 1;
            }
        }

        let total = start.elapsed();
        println!("Total: {:?}", total);
        println!("Frames processed: {}", frame_count);
        if frame_count > 0 {
            println!(
                "Avg capture: {:?}, Avg encode: {:?}",
                capture_time / frame_count as u32,
                encode_time / frame_count as u32
            );
            println!("FPS: {:.1}\n", frame_count as f64 / total.as_secs_f64());
        }
    }

    Ok(())
}
