//! Optimized screen recording test with async capture.
//!
//! Captures screen at 30fps, encodes to H.265 (HEVC) using VideoToolbox,
//! saves to disk for 1 minute, and monitors performance.
//!
//! Optimization: Uses separate threads for capture and encode with
//! a channel-based producer-consumer pattern to overlap operations.

use std::fs::File;
use std::io::Write;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};
use waterkit_codec::{CodecType, Frame, PixelFormat, VideoEncoder};

const TARGET_FPS: f64 = 30.0;
const FRAME_INTERVAL: Duration = Duration::from_nanos((1_000_000_000.0 / TARGET_FPS) as u64);
const RECORDING_DURATION: Duration = Duration::from_secs(60);
const OUTPUT_FILE: &str = "screen_recording_optimized.h265";
const BUFFER_SIZE: usize = 4; // Number of frames to buffer

struct CapturedFrame {
    frame: Frame,
    capture_time_ms: f64,
}

struct PerformanceStats {
    total_frames: usize,
    successful_frames: usize,
    total_bytes: usize,
    capture_time_ms: Vec<f64>,
    encode_time_ms: Vec<f64>,
    dropped_frames: usize,
    queue_full_drops: usize,
}

impl PerformanceStats {
    fn new() -> Self {
        Self {
            total_frames: 0,
            successful_frames: 0,
            total_bytes: 0,
            capture_time_ms: Vec::with_capacity(1800),
            encode_time_ms: Vec::with_capacity(1800),
            dropped_frames: 0,
            queue_full_drops: 0,
        }
    }

    fn avg(&self, times: &[f64]) -> f64 {
        if times.is_empty() {
            0.0
        } else {
            times.iter().sum::<f64>() / times.len() as f64
        }
    }

    fn max(&self, times: &[f64]) -> f64 {
        times.iter().cloned().fold(0.0, f64::max)
    }

    fn percentile(&self, times: &[f64], p: usize) -> f64 {
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
        println!(
            "Dropped:        {} (queue full: {})",
            self.dropped_frames, self.queue_full_drops
        );
        println!("Actual FPS:     {:.2}", actual_fps);
        println!(
            "Output size:    {:.2} MB",
            self.total_bytes as f64 / 1_000_000.0
        );
        println!("Bitrate:        {:.2} Mbps", bitrate_mbps);
        println!("\n-- Capture Times --");
        println!("  Average:      {:.2} ms", self.avg(&self.capture_time_ms));
        println!(
            "  P95:          {:.2} ms",
            self.percentile(&self.capture_time_ms, 95)
        );
        println!("  Max:          {:.2} ms", self.max(&self.capture_time_ms));
        println!("\n-- Encode Times --");
        println!("  Average:      {:.2} ms", self.avg(&self.encode_time_ms));
        println!(
            "  P95:          {:.2} ms",
            self.percentile(&self.encode_time_ms, 95)
        );
        println!("  Max:          {:.2} ms", self.max(&self.encode_time_ms));
        println!("\n-- Throughput --");
        let total_pipeline = self.avg(&self.capture_time_ms) + self.avg(&self.encode_time_ms);
        println!(
            "  Max theoretical FPS (sequential): {:.1}",
            1000.0 / total_pipeline
        );
        println!(
            "  Max theoretical FPS (pipelined):  {:.1}",
            1000.0
                / self
                    .avg(&self.capture_time_ms)
                    .max(self.avg(&self.encode_time_ms))
        );
        println!("=================================================");
    }
}

// Capture thread function
fn capture_thread(
    tx: mpsc::SyncSender<CapturedFrame>,
    width: u32,
    height: u32,
    duration: Duration,
) {
    let start_time = Instant::now();
    let mut next_frame_time = Instant::now();
    let mut frame_number = 0u64;

    while start_time.elapsed() < duration {
        let capture_start = Instant::now();

        match waterkit_screen::capture_screen_raw(0) {
            Ok(raw) => {
                let capture_time = capture_start.elapsed().as_secs_f64() * 1000.0;

                if raw.width != width || raw.height != height {
                    continue; // Skip if dimensions changed
                }

                let frame = Frame {
                    data: Arc::new(raw.data),
                    width: raw.width,
                    height: raw.height,
                    format: PixelFormat::Rgba,
                    timestamp_ns: frame_number * (1_000_000_000 / TARGET_FPS as u64),
                };

                // Non-blocking send - drop frame if buffer is full
                match tx.try_send(CapturedFrame {
                    frame,
                    capture_time_ms: capture_time,
                }) {
                    Ok(_) => frame_number += 1,
                    Err(mpsc::TrySendError::Full(_)) => {
                        // Buffer full, skip this frame
                    }
                    Err(mpsc::TrySendError::Disconnected(_)) => break,
                }
            }
            Err(_) => continue,
        }

        // Rate limiting
        next_frame_time += FRAME_INTERVAL;
        let now = Instant::now();
        if next_frame_time > now {
            thread::sleep(next_frame_time - now);
        } else if now - next_frame_time > FRAME_INTERVAL * 2 {
            next_frame_time = now; // Reset if too far behind
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    println!("=================================================");
    println!("   Optimized Screen Recording Test");
    println!("   H.265 @ 30fps for 60 seconds");
    println!("   Using async capture pipeline");
    println!("=================================================");

    // Get screen info
    let screens = waterkit_screen::screens()?;
    let primary = screens.iter().find(|s| s.is_primary).unwrap_or(&screens[0]);

    // First capture to get actual dimensions
    println!("\nInitializing...");
    let initial_capture = waterkit_screen::capture_screen_raw(0)?;
    let width = initial_capture.width;
    let height = initial_capture.height;
    println!("Screen: {} ({}x{})", primary.name, width, height);

    // Create encoder
    println!("Creating H.265 encoder...");
    let mut encoder = waterkit_codec::sys::AppleEncoder::with_size(CodecType::H265, width, height)
        .map_err(|e| format!("Failed to create encoder: {:?}", e))?;
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
        // Try to receive with timeout
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(captured) => {
                stats.total_frames += 1;
                stats.capture_time_ms.push(captured.capture_time_ms);

                // Encode
                let encode_start = Instant::now();
                match encoder.encode(&captured.frame) {
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
                        stats.dropped_frames += 1;
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
            let bar = "█".repeat((progress / 2).min(50));
            let remaining = " ".repeat(50 - (progress / 2).min(50));
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
