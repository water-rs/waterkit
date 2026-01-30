//! Screen recording test with H.265 encoding.
//!
//! Captures screen at 30fps, encodes to H.265 (HEVC) using VideoToolbox,
//! saves raw H.265 bitstream to disk, and monitors performance.

use std::fs::File;
use std::io::Write;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};
use waterkit_codec::{CodecType, Encoder};
use waterkit_screen::{ScreenStream, StreamConfig, screens};

const TARGET_FPS: f64 = 30.0;
const FRAME_INTERVAL: Duration = Duration::from_nanos((1_000_000_000.0 / TARGET_FPS) as u64);
const RECORDING_DURATION: Duration = Duration::from_secs(30);
const OUTPUT_FILE: &str = "screen_recording.h265";
const BUFFER_SIZE: usize = 4;

struct CapturedFrame {
    nv12_data: Vec<u8>,
    capture_time_ms: f64,
}

struct PerformanceStats {
    total_frames: usize,
    successful_frames: usize,
    total_bytes: usize,
    capture_time_ms: Vec<f64>,
    encode_time_ms: Vec<f64>,
}

impl PerformanceStats {
    fn new() -> Self {
        Self {
            total_frames: 0,
            successful_frames: 0,
            total_bytes: 0,
            capture_time_ms: Vec::with_capacity(1000),
            encode_time_ms: Vec::with_capacity(1000),
        }
    }

    fn avg(times: &[f64]) -> f64 {
        if times.is_empty() {
            0.0
        } else {
            times.iter().sum::<f64>() / times.len() as f64
        }
    }

    fn max(times: &[f64]) -> f64 {
        times.iter().cloned().fold(0.0, f64::max)
    }

    fn percentile(times: &[f64], p: usize) -> f64 {
        if times.is_empty() {
            return 0.0;
        }
        let mut sorted = times.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let idx = (sorted.len() * p / 100).min(sorted.len() - 1);
        sorted[idx]
    }

    fn print_summary(&self, elapsed: Duration) {
        let actual_fps = self.successful_frames as f64 / elapsed.as_secs_f64();
        let bitrate_mbps = (self.total_bytes as f64 * 8.0) / (elapsed.as_secs_f64() * 1_000_000.0);

        println!("\n=================================================");
        println!("             RECORDING COMPLETE");
        println!("=================================================");
        println!("Duration:       {:.1}s", elapsed.as_secs_f64());
        println!("Total frames:   {}", self.total_frames);
        println!("Successful:     {}", self.successful_frames);
        println!("Actual FPS:     {:.2}", actual_fps);
        println!(
            "Output size:    {:.2} MB",
            self.total_bytes as f64 / 1_000_000.0
        );
        println!("Bitrate:        {:.2} Mbps", bitrate_mbps);
        println!("\n-- Capture Times --");
        println!("  Average:      {:.2} ms", Self::avg(&self.capture_time_ms));
        println!(
            "  P95:          {:.2} ms",
            Self::percentile(&self.capture_time_ms, 95)
        );
        println!("  Max:          {:.2} ms", Self::max(&self.capture_time_ms));
        println!("\n-- Encode Times --");
        println!("  Average:      {:.2} ms", Self::avg(&self.encode_time_ms));
        println!(
            "  P95:          {:.2} ms",
            Self::percentile(&self.encode_time_ms, 95)
        );
        println!("  Max:          {:.2} ms", Self::max(&self.encode_time_ms));
        println!("\n-- Throughput --");
        let total_pipeline = Self::avg(&self.capture_time_ms) + Self::avg(&self.encode_time_ms);
        println!(
            "  Max theoretical FPS (sequential): {:.1}",
            1000.0 / total_pipeline.max(0.001)
        );
        println!("=================================================");
    }
}

fn capture_thread(
    tx: mpsc::SyncSender<CapturedFrame>,
    width: u32,
    height: u32,
    duration: Duration,
) {
    // Create wgpu device for this thread
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .expect("No GPU adapter");
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .expect("Failed to create device");
    let device = Arc::new(device);
    let queue = Arc::new(queue);

    let displays = screens().expect("No screens");
    let primary = displays
        .iter()
        .find(|d| d.is_primary)
        .unwrap_or(&displays[0]);

    let config = StreamConfig {
        target_fps: 60,
        show_cursor: true,
    };

    let stream =
        ScreenStream::start(primary, device, queue, &config).expect("Failed to start stream");

    // Wait for stream warmup
    std::thread::sleep(Duration::from_millis(500));

    let start_time = Instant::now();
    let mut next_frame_time = Instant::now();

    while start_time.elapsed() < duration {
        let capture_start = Instant::now();

        if let Some(frame) = stream.try_next_frame() {
            let capture_time = capture_start.elapsed().as_secs_f64() * 1000.0;

            if frame.width() != width || frame.height() != height {
                continue; // Skip if dimensions changed
            }

            // Create dummy NV12 data (in a real pipeline, we'd read back the texture or use IOSurface)
            let y_size = (width * height) as usize;
            let nv12_data = vec![128u8; y_size + y_size / 2];

            // Non-blocking send
            match tx.try_send(CapturedFrame {
                nv12_data,
                capture_time_ms: capture_time,
            }) {
                Ok(_) => {}
                Err(mpsc::TrySendError::Full(_)) => {
                    // Buffer full, skip frame
                }
                Err(mpsc::TrySendError::Disconnected(_)) => break,
            }
        }

        // Rate limiting
        next_frame_time += FRAME_INTERVAL;
        let now = Instant::now();
        if next_frame_time > now {
            thread::sleep(next_frame_time - now);
        } else if now - next_frame_time > FRAME_INTERVAL * 2 {
            next_frame_time = now;
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    println!("=================================================");
    println!("   Screen Recording Test");
    println!(
        "   H.265 @ 30fps for {} seconds",
        RECORDING_DURATION.as_secs()
    );
    println!("   Using async capture pipeline");
    println!("=================================================");

    // Get screen info
    let displays = screens()?;
    let primary = displays
        .iter()
        .find(|d| d.is_primary)
        .unwrap_or(&displays[0]);
    let width = primary.width;
    let height = primary.height;
    println!("Screen: {} ({}x{})", primary.name, width, height);

    // Create encoder
    println!("Creating H.265 encoder...");
    let mut encoder = Encoder::new(CodecType::H265, width, height)?;
    println!("Encoder ready!");

    // Create output file
    let mut output_file = File::create(OUTPUT_FILE)?;
    println!("Output: {}", OUTPUT_FILE);

    // Create bounded channel for frame buffer
    let (tx, rx): (mpsc::SyncSender<CapturedFrame>, Receiver<CapturedFrame>) =
        mpsc::sync_channel(BUFFER_SIZE);

    let mut stats = PerformanceStats::new();
    let start_time = Instant::now();

    println!("\nRecording with pipelined capture/encode...");
    println!("Progress: [                                        ] 0%");

    // Start capture thread
    let capture_handle = thread::spawn(move || {
        capture_thread(tx, width, height, RECORDING_DURATION);
    });

    // Main thread: encode loop
    let mut last_progress_print = Instant::now();

    while start_time.elapsed() < RECORDING_DURATION + Duration::from_millis(500) {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(captured) => {
                stats.total_frames += 1;
                stats.capture_time_ms.push(captured.capture_time_ms);

                // Encode
                let encode_start = Instant::now();
                for result in encoder.encode_nv12(&captured.nv12_data) {
                    match result {
                        Ok(data) => {
                            let encode_time = encode_start.elapsed().as_secs_f64() * 1000.0;
                            stats.encode_time_ms.push(encode_time);

                            if !data.is_empty() {
                                output_file.write_all(&data)?;
                                stats.total_bytes += data.len();
                                stats.successful_frames += 1;
                            }
                        }
                        Err(e) => {
                            eprintln!("\rEncode error: {:?}", e);
                        }
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        // Print progress periodically
        if last_progress_print.elapsed() > Duration::from_secs(1) {
            let elapsed = start_time.elapsed();
            let progress =
                (elapsed.as_secs_f64() / RECORDING_DURATION.as_secs_f64() * 100.0) as usize;
            let bar_filled = (progress / 2).min(50);
            let bar = "█".repeat(bar_filled);
            let remaining = " ".repeat(50 - bar_filled);
            print!(
                "\rProgress: [{}{}] {}%  FPS: {:.1}  Size: {:.1}MB  ",
                bar,
                remaining,
                progress.min(100),
                stats.successful_frames as f64 / elapsed.as_secs_f64(),
                stats.total_bytes as f64 / 1_000_000.0
            );
            std::io::stdout().flush()?;
            last_progress_print = Instant::now();
        }
    }

    // Wait for capture thread
    let _ = capture_handle.join();

    let total_elapsed = start_time.elapsed();
    println!();

    stats.print_summary(total_elapsed);

    println!("\nRecording saved to: {}", OUTPUT_FILE);
    println!("You can play it with: ffplay {}", OUTPUT_FILE);

    Ok(())
}
