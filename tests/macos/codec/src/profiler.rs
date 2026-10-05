//! Quick profiling test for screen capture performance.
//!
//! Tests screenshot latency and GPU streaming capture throughput.

use std::sync::Arc;
use std::time::{Duration, Instant};
use waterkit_codec::{CodecType, Encoder, EncoderProfile};
use waterkit_screen::{ImageFormat, ScreenInfo, ScreenStream, StreamConfig, screens, screenshot};

const ITERATIONS: u32 = 100;

type BoxError = Box<dyn std::error::Error>;

fn main() -> Result<(), BoxError> {
    env_logger::init();

    println!("Screen Capture Performance Test ({ITERATIONS} iterations each)\n");

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

    measure_screenshots(primary)?;
    measure_gpu_streaming(primary)?;
    measure_streaming_encode(primary)?;

    Ok(())
}

/// Creates a GPU device and queue for a capture stream.
fn gpu_device() -> Result<(Arc<wgpu::Device>, Arc<wgpu::Queue>), BoxError> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
    Ok((Arc::new(device), Arc::new(queue)))
}

/// Screenshot capture with PNG encoding.
fn measure_screenshots(primary: &ScreenInfo) -> Result<(), BoxError> {
    println!("=== Test 1: Screenshot Capture (PNG) ===");
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        let _ = screenshot(primary, ImageFormat::Png)?;
    }
    let total = start.elapsed();
    println!(
        "Total: {total:?}, Avg: {:?}/frame, FPS: {:.1}\n",
        total / ITERATIONS,
        f64::from(ITERATIONS) / total.as_secs_f64()
    );
    Ok(())
}

/// GPU streaming capture throughput.
fn measure_gpu_streaming(primary: &ScreenInfo) -> Result<(), BoxError> {
    println!("=== Test 2: GPU Streaming Capture ===");
    let (device, queue) = gpu_device()?;
    let config = StreamConfig {
        target_fps: 120,
        show_cursor: false,
    };
    let stream = ScreenStream::start(primary, device, queue, &config)?;

    // Wait for stream to warm up
    std::thread::sleep(Duration::from_millis(500));

    println!("Running 5-second capture test...");
    let duration = Duration::from_secs(5);
    let start = Instant::now();
    let mut frame_count = 0u32;

    while start.elapsed() < duration {
        if stream.try_next_frame().is_some() {
            frame_count += 1;
        }
    }

    let total = start.elapsed();
    let fps = f64::from(frame_count) / total.as_secs_f64();
    println!("Duration: {total:?}");
    println!("Frames captured: {frame_count}");
    println!("**GPU Streaming FPS: {fps:.1}**\n");
    Ok(())
}

/// GPU streaming capture followed by H.265 encoding.
fn measure_streaming_encode(primary: &ScreenInfo) -> Result<(), BoxError> {
    println!("=== Test 3: GPU Streaming + H.265 Encode ===");
    let (device, queue) = gpu_device()?;
    let config = StreamConfig {
        target_fps: 60,
        show_cursor: false,
    };
    let stream = ScreenStream::start(primary, device, queue, &config)?;
    let (width, height) = stream.dimensions();

    let mut encoder = Encoder::new(CodecType::H265, width, height, EncoderProfile::Offline)?;

    // Wait for stream to warm up
    std::thread::sleep(Duration::from_millis(500));

    let start = Instant::now();
    let mut capture_time = Duration::ZERO;
    let mut encode_time = Duration::ZERO;
    let mut frame_count = 0u32;

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
    println!("Total: {total:?}");
    println!("Frames processed: {frame_count}");
    if frame_count > 0 {
        println!(
            "Avg capture: {:?}, Avg encode: {:?}",
            capture_time / frame_count,
            encode_time / frame_count
        );
        println!("FPS: {:.1}\n", f64::from(frame_count) / total.as_secs_f64());
    }
    Ok(())
}
