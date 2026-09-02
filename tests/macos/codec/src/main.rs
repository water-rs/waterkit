//! Performance benchmark for waterkit-codec.
//!
//! Tests encoding performance using hardware accelerated (Apple VideoToolbox) encoders.
//! Measures throughput with screen capture as input source.

use std::time::Instant;
use waterkit_codec::{CodecType, Encoder};

fn create_test_nv12(width: u32, height: u32) -> Vec<u8> {
    // Create a dummy NV12 frame for testing
    // Y plane: width * height bytes
    // UV plane: width * height / 2 bytes (interleaved)
    let y_size = (width * height) as usize;
    let uv_size = y_size / 2;
    let mut data = vec![128u8; y_size + uv_size]; // Flat grey

    // Fill Y plane with gradient
    for y in 0..height as usize {
        for x in 0..width as usize {
            data[y * width as usize + x] = ((x + y) % 256) as u8;
        }
    }

    data
}

fn benchmark_encoder(
    name: &str,
    encoder: &mut Encoder,
    nv12_data: &[u8],
    iterations: usize,
) -> BenchResult {
    println!("\n=== Benchmarking {} ===", name);

    // Warmup
    for _ in 0..5 {
        for result in encoder.encode_nv12(nv12_data) {
            let _ = result;
        }
    }

    // Timed run
    let start = Instant::now();
    let mut success_count = 0;
    let mut total_bytes = 0usize;

    for _ in 0..iterations {
        for result in encoder.encode_nv12(nv12_data) {
            match result {
                Ok(data) => {
                    success_count += 1;
                    total_bytes += data.len();
                }
                Err(e) => {
                    eprintln!("Encode error: {:?}", e);
                }
            }
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

    BenchResult {
        name: name.to_string(),
        fps,
        frame_time_ms,
        success_count,
        iterations,
    }
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
    println!("   Hardware Encoding (VideoToolbox)");
    println!("=================================================");

    let mut results: Vec<BenchResult> = Vec::new();

    // =====================================================
    // PHASE 1: Camera-like input (1080p, typical webcam)
    // =====================================================
    println!("\n>>> PHASE 1: Camera Input (1080p)");
    {
        let nv12_data = create_test_nv12(1920, 1080);

        // VideoToolbox H.264
        println!("\n--- Hardware H.264 (VideoToolbox) ---");
        match Encoder::new(CodecType::H264, 1920, 1080) {
            Ok(mut encoder) => {
                results.push(benchmark_encoder(
                    "H.264 VT (1080p)",
                    &mut encoder,
                    &nv12_data,
                    100,
                ));
            }
            Err(e) => println!("  Failed: {:?}", e),
        }

        // VideoToolbox H.265
        println!("\n--- Hardware H.265 (VideoToolbox) ---");
        match Encoder::new(CodecType::H265, 1920, 1080) {
            Ok(mut encoder) => {
                results.push(benchmark_encoder(
                    "H.265 VT (1080p)",
                    &mut encoder,
                    &nv12_data,
                    100,
                ));
            }
            Err(e) => println!("  Failed: {:?}", e),
        }
    }

    // =====================================================
    // PHASE 2: Screen capture input (4K, high pressure)
    // =====================================================
    println!("\n>>> PHASE 2: Screen Capture (High Pressure - 4K)");

    // Try to get actual screen resolution
    let (screen_width, screen_height) = match waterkit_screen::screens() {
        Ok(screens) if !screens.is_empty() => {
            let primary = screens
                .iter()
                .find(|s| s.is_primary())
                .unwrap_or(&screens[0]);
            println!(
                "  Using screen: {} ({}x{})",
                primary.name(),
                primary.width(),
                primary.height()
            );
            (primary.width(), primary.height())
        }
        _ => {
            println!("  No screen info available, using 4K default");
            (3840, 2160)
        }
    };

    let nv12_data = create_test_nv12(screen_width, screen_height);

    // VideoToolbox H.264 on 4K
    println!("\n--- Hardware H.264 (VideoToolbox) on Screen Size ---");
    match Encoder::new(CodecType::H264, screen_width, screen_height) {
        Ok(mut encoder) => {
            results.push(benchmark_encoder(
                "H.264 VT (4K)",
                &mut encoder,
                &nv12_data,
                50,
            ));
        }
        Err(e) => println!("  Failed: {:?}", e),
    }

    // VideoToolbox H.265 on 4K
    println!("\n--- Hardware H.265 (VideoToolbox) on Screen Size ---");
    match Encoder::new(CodecType::H265, screen_width, screen_height) {
        Ok(mut encoder) => {
            results.push(benchmark_encoder(
                "H.265 VT (4K)",
                &mut encoder,
                &nv12_data,
                50,
            ));
        }
        Err(e) => println!("  Failed: {:?}", e),
    }

    // =====================================================
    // SUMMARY
    // =====================================================
    println!("\n=================================================");
    println!("                  SUMMARY");
    println!("=================================================");
    println!(
        "{:<20} {:>10} {:>12} {:>10}",
        "Encoder", "FPS", "Frame(ms)", "Success"
    );
    println!("-------------------------------------------------");
    for r in &results {
        println!(
            "{:<20} {:>10.1} {:>12.2} {:>7}/{}",
            r.name, r.fps, r.frame_time_ms, r.success_count, r.iterations
        );
    }
    println!("=================================================");
}
