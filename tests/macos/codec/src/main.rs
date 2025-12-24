//! Performance benchmark for waterkit-codec.
//!
//! Tests encoding performance using:
//! - Camera input (low pressure - 1080p@30fps typical)
//! - Screen capture input (high pressure - 4K@120fps capable)
//!
//! Measures both hardware accelerated (Apple VideoToolbox) and software (AV1/rav1e) encoders.

use std::sync::Arc;
use std::time::{Duration, Instant};
use waterkit_codec::{Frame, PixelFormat, CodecType, VideoEncoder};

fn create_test_frame(width: u32, height: u32) -> Frame {
    // Create a dummy RGBA frame for testing
    let size = (width * height * 4) as usize;
    let data = vec![128u8; size]; // Flat grey
    Frame {
        data: Arc::new(data),
        width,
        height,
        format: PixelFormat::Rgba,
        timestamp_ns: 0,
    }
}

fn benchmark_encoder<E: VideoEncoder>(name: &str, encoder: &mut E, frame: &Frame, iterations: usize) -> BenchResult {
    println!("\n=== Benchmarking {} ===", name);
    
    // Warmup
    for _ in 0..5 {
        let _ = encoder.encode(frame);
    }
    
    // Timed run
    let start = Instant::now();
    let mut success_count = 0;
    let mut total_bytes = 0usize;
    
    for _ in 0..iterations {
        match encoder.encode(frame) {
            Ok(data) => {
                success_count += 1;
                total_bytes += data.len();
            }
            Err(_) => {}
        }
    }
    
    let elapsed = start.elapsed();
    let fps = iterations as f64 / elapsed.as_secs_f64();
    let frame_time_ms = elapsed.as_secs_f64() * 1000.0 / iterations as f64;
    
    println!("  Iterations: {}", iterations);
    println!("  Successful: {}", success_count);
    println!("  Total time: {:?}", elapsed);
    println!("  FPS: {:.1}", fps);
    println!("  Frame time: {:.2} ms", frame_time_ms);
    if total_bytes > 0 {
        let mbps = (total_bytes as f64 * 8.0) / (elapsed.as_secs_f64() * 1_000_000.0);
        println!("  Output bitrate: {:.2} Mbps", mbps);
    }
    
    BenchResult { name: name.to_string(), fps, frame_time_ms, success_count, iterations }
}

struct BenchResult {
    name: String,
    fps: f64,
    frame_time_ms: f64,
    success_count: usize,
    iterations: usize,
}

fn main() {
    env_logger::init();
    
    println!("=================================================");
    println!("   Codec Performance Benchmark");
    println!("   Camera (Low Pressure) + Screen (High Pressure)");
    println!("=================================================");
    
    let mut results: Vec<BenchResult> = Vec::new();
    
    // =====================================================
    // PHASE 1: Camera-like input (1080p, typical webcam)
    // =====================================================
    println!("\n>>> PHASE 1: Camera Input (Low Pressure - 1080p)");
    {
        let frame = create_test_frame(1920, 1080);
        
        // AV1 software
        println!("\n--- Software AV1 (rav1e) ---");
        match waterkit_codec::av1::Av1Encoder::new(1920, 1080) {
            Ok(mut encoder) => {
                results.push(benchmark_encoder("AV1 (1080p)", &mut encoder, &frame, 10));
            }
            Err(e) => println!("  Failed: {:?}", e),
        }
        
        // VideoToolbox H.264
        #[cfg(target_vendor = "apple")]
        {
            println!("\n--- Hardware H.264 (VideoToolbox) ---");
            match waterkit_codec::sys::AppleEncoder::new(CodecType::H264) {
                Ok(mut encoder) => {
                    results.push(benchmark_encoder("H.264 VT (1080p)", &mut encoder, &frame, 100));
                }
                Err(e) => println!("  Failed: {:?}", e),
            }
        }
    }
    
    // =====================================================
    // PHASE 2: Screen capture input (4K, high pressure)
    // =====================================================
    println!("\n>>> PHASE 2: Screen Capture (High Pressure - 4K)");
    
    // Try to get actual screen resolution
    let (screen_width, screen_height) = match waterkit_screen::screens() {
        Ok(screens) if !screens.is_empty() => {
            let primary = screens.iter().find(|s| s.is_primary).unwrap_or(&screens[0]);
            println!("  Using screen: {} ({}x{})", primary.name, primary.width, primary.height);
            (primary.width, primary.height)
        }
        _ => {
            println!("  No screen info available, using 4K default");
            (3840, 2160)
        }
    };
    
    // Benchmark screen capture speed itself
    println!("\n--- Screen Capture Latency ---");
    {
        let start = Instant::now();
        let iterations = 10;
        for _ in 0..iterations {
            let _ = waterkit_screen::capture_screen_raw(0);
        }
        let elapsed = start.elapsed();
        let fps = iterations as f64 / elapsed.as_secs_f64();
        println!("  Screen capture: {:.1} FPS ({:.2} ms/frame)", fps, elapsed.as_secs_f64() * 1000.0 / iterations as f64);
    }
    
    // Get a real screen frame for encoding test
    let screen_frame = match waterkit_screen::capture_screen_raw(0) {
        Ok(raw) => {
            println!("  Captured screen: {}x{}", raw.width, raw.height);
            Frame {
                data: Arc::new(raw.data),
                width: raw.width,
                height: raw.height,
                format: PixelFormat::Rgba,
                timestamp_ns: 0,
            }
        }
        Err(e) => {
            println!("  Screen capture failed: {:?}, using synthetic 4K frame", e);
            create_test_frame(screen_width, screen_height)
        }
    };
    
    // AV1 on 4K
    println!("\n--- Software AV1 (rav1e) on 4K ---");
    match waterkit_codec::av1::Av1Encoder::new(screen_frame.width as usize, screen_frame.height as usize) {
        Ok(mut encoder) => {
            results.push(benchmark_encoder("AV1 (4K)", &mut encoder, &screen_frame, 5));
        }
        Err(e) => println!("  Failed: {:?}", e),
    }
    
    // VideoToolbox H.264 on 4K
    #[cfg(target_vendor = "apple")]
    {
        println!("\n--- Hardware H.264 (VideoToolbox) on 4K ---");
        match waterkit_codec::sys::AppleEncoder::with_size(CodecType::H264, screen_frame.width, screen_frame.height) {
            Ok(mut encoder) => {
                results.push(benchmark_encoder("H.264 VT (4K)", &mut encoder, &screen_frame, 50));
            }
            Err(e) => println!("  Failed: {:?}", e),
        }
        
        println!("\n--- Hardware H.265 (VideoToolbox) on 4K ---");
        match waterkit_codec::sys::AppleEncoder::with_size(CodecType::H265, screen_frame.width, screen_frame.height) {
            Ok(mut encoder) => {
                results.push(benchmark_encoder("H.265 VT (4K)", &mut encoder, &screen_frame, 50));
            }
            Err(e) => println!("  Failed: {:?}", e),
        }
    }
    
    // =====================================================
    // SUMMARY
    // =====================================================
    println!("\n=================================================");
    println!("                  SUMMARY");
    println!("=================================================");
    println!("{:<20} {:>10} {:>12} {:>10}", "Encoder", "FPS", "Frame(ms)", "Success");
    println!("-------------------------------------------------");
    for r in &results {
        println!("{:<20} {:>10.1} {:>12.2} {:>7}/{}", r.name, r.fps, r.frame_time_ms, r.success_count, r.iterations);
    }
    println!("=================================================");
}
