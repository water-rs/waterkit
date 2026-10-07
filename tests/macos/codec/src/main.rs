//! Performance benchmark for waterkit-codec.
//!
//! Tests encoding performance using hardware accelerated (Apple `VideoToolbox`) encoders.
//! Measures throughput with screen capture as input source.

mod common;

use std::time::Instant;
use waterkit_codec::{CodecType, Encoder, EncoderProfile};

fn create_test_nv12(width: u32, height: u32) -> Vec<u8> {
    // Create a dummy NV12 frame for testing
    // Y plane: width * height bytes
    // UV plane: width * height / 2 bytes (interleaved)
    let width = width as usize;
    let y_size = width * height as usize;
    let uv_size = y_size / 2;
    let mut data = vec![128u8; y_size + uv_size]; // Flat grey

    // Fill Y plane with a diagonal gradient that wraps every 256 steps.
    for (y, row) in data[..y_size].chunks_exact_mut(width).enumerate() {
        for (x, luma) in row.iter_mut().enumerate() {
            *luma = u8::try_from((x + y) % 256).expect("a value reduced modulo 256 fits in u8");
        }
    }

    data
}

/// Converts a byte count to megabits for a printed bitrate.
#[expect(
    clippy::cast_precision_loss,
    reason = "a benchmark's encoded byte total stays far below 2^52, where the conversion is exact"
)]
fn megabits(bytes: usize) -> f64 {
    bytes as f64 * 8.0 / 1_000_000.0
}

fn benchmark_encoder(
    name: &str,
    encoder: &mut Encoder,
    nv12_data: &[u8],
    iterations: u32,
) -> BenchResult {
    println!("\n=== Benchmarking {name} ===");

    // Warmup
    for _ in 0..5 {
        for result in encoder.encode_nv12(nv12_data) {
            let _ = result;
        }
    }

    // Timed run
    let start = Instant::now();
    let mut success_count = 0u32;
    let mut total_bytes = 0usize;

    for _ in 0..iterations {
        for result in encoder.encode_nv12(nv12_data) {
            match result {
                Ok(data) => {
                    success_count += 1;
                    total_bytes += data.len();
                }
                Err(e) => {
                    eprintln!("Encode error: {e:?}");
                }
            }
        }
    }

    let elapsed = start.elapsed();
    let fps = f64::from(iterations) / elapsed.as_secs_f64();
    let frame_time_ms = elapsed.as_secs_f64() * 1000.0 / f64::from(iterations);

    println!("  Iterations: {iterations}");
    println!("  Successful: {success_count}");
    println!("  Total time: {elapsed:?}");
    println!("  FPS: {fps:.1}");
    println!("  Frame time: {frame_time_ms:.2} ms");
    if total_bytes > 0 {
        let mbps = megabits(total_bytes) / elapsed.as_secs_f64();
        println!("  Output bitrate: {mbps:.2} Mbps");
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
    success_count: u32,
    iterations: u32,
}

/// One encoder configuration to benchmark against one input frame.
struct BenchCase<'a> {
    name: &'a str,
    codec: CodecType,
    width: u32,
    height: u32,
    profile: EncoderProfile,
    iterations: u32,
}

fn run_case(results: &mut Vec<BenchResult>, case: &BenchCase<'_>, nv12_data: &[u8]) {
    match Encoder::new(
        case.codec,
        case.width,
        case.height,
        case.profile,
        common::bt709_sdr_limited(),
    ) {
        Ok(mut encoder) => {
            results.push(benchmark_encoder(
                case.name,
                &mut encoder,
                nv12_data,
                case.iterations,
            ));
        }
        Err(e) => println!("  Failed: {e:?}"),
    }
}

/// The resolution of the primary screen, or 4K when no screen is reported.
fn screen_size() -> (u32, u32) {
    match waterkit_screen::screens() {
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
    }
}

fn print_summary(results: &[BenchResult]) {
    println!("\n=================================================");
    println!("                  SUMMARY");
    println!("=================================================");
    println!(
        "{:<20} {:>10} {:>12} {:>10}",
        "Encoder", "FPS", "Frame(ms)", "Success"
    );
    println!("-------------------------------------------------");
    for r in results {
        println!(
            "{:<20} {:>10.1} {:>12.2} {:>7}/{}",
            r.name, r.fps, r.frame_time_ms, r.success_count, r.iterations
        );
    }
    println!("=================================================");
}

fn main() {
    env_logger::init();

    println!("=================================================");
    println!("   Codec Performance Benchmark");
    println!("   Hardware Encoding (VideoToolbox)");
    println!("=================================================");

    let mut results: Vec<BenchResult> = Vec::new();

    // Camera-like input (1080p, typical webcam)
    println!("\n>>> Camera Input (1080p)");
    let nv12_data = create_test_nv12(1920, 1080);
    for (label, name, codec) in [
        ("H.264", "H.264 VT (1080p)", CodecType::H264),
        ("H.265", "H.265 VT (1080p)", CodecType::H265),
    ] {
        println!("\n--- Hardware {label} (VideoToolbox) ---");
        let case = BenchCase {
            name,
            codec,
            width: 1920,
            height: 1080,
            profile: EncoderProfile::Offline,
            iterations: 100,
        };
        run_case(&mut results, &case, &nv12_data);
    }

    // Screen capture input (4K, high pressure)
    println!("\n>>> Screen Capture (High Pressure - 4K)");
    let (screen_width, screen_height) = screen_size();
    let nv12_data = create_test_nv12(screen_width, screen_height);
    for (label, name, codec) in [
        ("H.264", "H.264 VT (4K)", CodecType::H264),
        ("H.265", "H.265 VT (4K)", CodecType::H265),
    ] {
        println!("\n--- Hardware {label} (VideoToolbox) on Screen Size ---");
        let case = BenchCase {
            name,
            codec,
            width: screen_width,
            height: screen_height,
            profile: EncoderProfile::Realtime,
            iterations: 50,
        };
        run_case(&mut results, &case, &nv12_data);
    }

    print_summary(&results);
}
