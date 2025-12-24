//! Quick profiling test for screen capture bottleneck analysis.
//!
//! Compares legacy capture_screen_raw() vs optimized ScreenCapturer.

use std::sync::Arc;
use std::time::{Duration, Instant};
use waterkit_codec::{CodecType, Frame, PixelFormat, VideoEncoder};

const ITERATIONS: usize = 100;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "Screen Capture Optimization Test ({} iterations each)\n",
        ITERATIONS
    );

    // Get initial capture for dimensions
    let initial = waterkit_screen::capture_screen_raw(0)?;
    let width = initial.width;
    let height = initial.height;
    println!("Screen: {}x{}\n", width, height);

    // Test 1: Legacy capture_screen_raw (calls Screen::all() every time)
    println!("=== Test 1: capture_screen_raw (per-call Screen::all) ===");
    {
        let start = Instant::now();
        let mut total_capture = Duration::ZERO;

        for _ in 0..ITERATIONS {
            let t = Instant::now();
            let _ = waterkit_screen::capture_screen_raw(0)?;
            total_capture += t.elapsed();
        }

        let total = start.elapsed();
        println!(
            "Total: {:?}, Avg capture: {:?}/frame, FPS: {:.1}\n",
            total,
            total_capture / ITERATIONS as u32,
            ITERATIONS as f64 / total.as_secs_f64()
        );
    }

    // Test 2: Optimized ScreenCapturer (cached Screen handle)
    println!("=== Test 2: ScreenCapturer (cached handle) ===");
    {
        let capturer = waterkit_screen::ScreenCapturer::new(0)?;

        let start = Instant::now();
        let mut total_capture = Duration::ZERO;

        for _ in 0..ITERATIONS {
            let t = Instant::now();
            let _ = capturer.capture()?;
            total_capture += t.elapsed();
        }

        let total = start.elapsed();
        println!(
            "Total: {:?}, Avg capture: {:?}/frame, FPS: {:.1}\n",
            total,
            total_capture / ITERATIONS as u32,
            ITERATIONS as f64 / total.as_secs_f64()
        );
    }

    // Test 3: Full pipeline with encoding (old method)
    println!("=== Test 3: ScreenCapturer + H.265 Encode ===");
    {
        let capturer = waterkit_screen::ScreenCapturer::new(0)?;
        let mut encoder =
            waterkit_codec::sys::AppleEncoder::with_size(CodecType::H265, width, height)?;

        let start = Instant::now();
        let mut total_capture = Duration::ZERO;
        let mut total_encode = Duration::ZERO;

        for _ in 0..50 {
            let t = Instant::now();
            let raw = capturer.capture()?;
            total_capture += t.elapsed();

            let frame = Frame {
                data: Arc::new(raw.data),
                width: raw.width,
                height: raw.height,
                format: PixelFormat::Rgba,
                timestamp_ns: 0,
            };

            let t = Instant::now();
            let _ = encoder.encode(&frame)?;
            total_encode += t.elapsed();
        }

        let total = start.elapsed();
        println!("Total: {:?}", total);
        println!(
            "Avg capture: {:?}, Avg encode: {:?}",
            total_capture / 50,
            total_encode / 50
        );
        println!("FPS: {:.1}\n", 50.0 / total.as_secs_f64());
    }

    // Test 4: ScreenCaptureKit streaming (SCKCapturer) - 120fps target
    println!("=== Test 4: SCKCapturer 120fps Capture (Zero-Copy IOSurface) ===");
    {
        match waterkit_screen::SCKCapturer::new() {
            Some(capturer) => {
                // Wait for stream to start producing frames
                std::thread::sleep(Duration::from_millis(500));

                // Reset counter and run timed test
                capturer.reset_frame_count();
                let start_seq = capturer.iosurface_sequence();

                println!("Running 5-second capture test...");
                let duration = Duration::from_secs(5);
                let start = Instant::now();
                let mut iosurface_reads = 0u64;

                // Test: poll IOSurface pointers as fast as possible
                while start.elapsed() < duration {
                    if capturer.iosurface_ptr().is_some() {
                        iosurface_reads += 1;
                    }
                }

                let total = start.elapsed();
                let frame_count = capturer.frame_count();
                let end_seq = capturer.iosurface_sequence();
                let unique_frames = end_seq - start_seq;

                let sck_fps = frame_count as f64 / total.as_secs_f64();
                let ios_fps = unique_frames as f64 / total.as_secs_f64();

                println!("Duration: {:?}", total);
                println!(
                    "Callback frames (frame_count): {} ({:.1} fps)",
                    frame_count, sck_fps
                );
                println!(
                    "IOSurface frames (sequence): {} ({:.1} fps)",
                    unique_frames, ios_fps
                );
                println!(
                    "IOSurface pointer reads: {} ({:.0}/sec)",
                    iosurface_reads,
                    iosurface_reads as f64 / total.as_secs_f64()
                );
                println!("**Zero-Copy IOSurface FPS: {:.1}**", ios_fps);
            }
            None => {
                println!("SCKCapturer not available (requires macOS 12.3+)");
            }
        }
    }

    Ok(())
}
