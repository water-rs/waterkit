//! Screen recording test with H.265 encoding.
//!
//! Captures screen at 30fps, encodes to H.265 (HEVC) using `VideoToolbox`,
//! saves raw H.265 bitstream to disk, and monitors performance.

use std::fs::File;
use std::io::Write;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};
use waterkit_codec::{CodecType, Encoder, EncoderProfile};
use waterkit_screen::{ScreenStream, StreamConfig, screens};

const TARGET_FPS: u64 = 30;
const FRAME_INTERVAL: Duration = Duration::from_nanos(1_000_000_000 / TARGET_FPS);
const RECORDING_DURATION: Duration = Duration::from_secs(30);
const OUTPUT_FILE: &str = "screen_recording.h265";
const BUFFER_SIZE: usize = 4;

struct CapturedFrame {
    nv12_data: Vec<u8>,
    capture_time: Duration,
}

struct PerformanceStats {
    total_frames: u32,
    successful_frames: u32,
    total_bytes: usize,
    capture_times: Vec<Duration>,
    encode_times: Vec<Duration>,
}

/// Converts a byte count to megabytes for a printed size or bitrate.
#[expect(
    clippy::cast_precision_loss,
    reason = "a recording's encoded byte total stays far below 2^52, where the conversion is exact"
)]
fn megabytes(bytes: usize) -> f64 {
    bytes as f64 / 1_000_000.0
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

impl PerformanceStats {
    fn new() -> Self {
        Self {
            total_frames: 0,
            successful_frames: 0,
            total_bytes: 0,
            capture_times: Vec::with_capacity(1000),
            encode_times: Vec::with_capacity(1000),
        }
    }

    fn avg(times: &[Duration]) -> Duration {
        if times.is_empty() {
            return Duration::ZERO;
        }
        let count = u32::try_from(times.len()).expect("a recording holds fewer than 2^32 samples");
        times.iter().sum::<Duration>() / count
    }

    fn max(times: &[Duration]) -> Duration {
        times.iter().copied().max().unwrap_or(Duration::ZERO)
    }

    fn percentile(times: &[Duration], p: usize) -> Duration {
        if times.is_empty() {
            return Duration::ZERO;
        }
        let mut sorted = times.to_vec();
        sorted.sort_unstable();
        let idx = (sorted.len() * p / 100).min(sorted.len() - 1);
        sorted[idx]
    }

    fn print_summary(&self, elapsed: Duration) {
        let seconds = elapsed.as_secs_f64();
        let actual_fps = f64::from(self.successful_frames) / seconds;
        let output_mb = megabytes(self.total_bytes);
        let bitrate_mbps = output_mb * 8.0 / seconds;

        println!("\n=================================================");
        println!("             RECORDING COMPLETE");
        println!("=================================================");
        println!("Duration:       {seconds:.1}s");
        println!("Total frames:   {}", self.total_frames);
        println!("Successful:     {}", self.successful_frames);
        println!("Actual FPS:     {actual_fps:.2}");
        println!("Output size:    {output_mb:.2} MB");
        println!("Bitrate:        {bitrate_mbps:.2} Mbps");
        println!("\n-- Capture Times --");
        Self::print_times(&self.capture_times);
        println!("\n-- Encode Times --");
        Self::print_times(&self.encode_times);
        println!("\n-- Throughput --");
        let total_pipeline = millis(Self::avg(&self.capture_times) + Self::avg(&self.encode_times));
        println!(
            "  Max theoretical FPS (sequential): {:.1}",
            1000.0 / total_pipeline.max(0.001)
        );
        println!("=================================================");
    }

    fn print_times(times: &[Duration]) {
        println!("  Average:      {:.2} ms", millis(Self::avg(times)));
        println!(
            "  P95:          {:.2} ms",
            millis(Self::percentile(times, 95))
        );
        println!("  Max:          {:.2} ms", millis(Self::max(times)));
    }
}

fn capture_thread(
    tx: &mpsc::SyncSender<CapturedFrame>,
    width: u32,
    height: u32,
    duration: Duration,
) {
    // Create wgpu device for this thread
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
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
        .find(|d| d.is_primary())
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
            let capture_time = capture_start.elapsed();

            if frame.width() != width || frame.height() != height {
                continue; // Skip if dimensions changed
            }

            // Create dummy NV12 data (in a real pipeline, we'd read back the texture or use IOSurface)
            let y_size = (width * height) as usize;
            let nv12_data = vec![128u8; y_size + y_size / 2];

            // Non-blocking send; a full buffer drops the frame.
            match tx.try_send(CapturedFrame {
                nv12_data,
                capture_time,
            }) {
                Ok(()) | Err(mpsc::TrySendError::Full(_)) => {}
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
        "   H.265 @ {TARGET_FPS}fps for {} seconds",
        RECORDING_DURATION.as_secs()
    );
    println!("   Using async capture pipeline");
    println!("=================================================");

    // Get screen info
    let displays = screens()?;
    let primary = displays
        .iter()
        .find(|d| d.is_primary())
        .unwrap_or(&displays[0]);
    let width = primary.width();
    let height = primary.height();
    println!("Screen: {} ({width}x{height})", primary.name());

    // Create encoder
    println!("Creating H.265 encoder...");
    let mut encoder = Encoder::new(CodecType::H265, width, height, EncoderProfile::Realtime)?;
    println!("Encoder ready!");

    // Create output file
    let mut output_file = File::create(OUTPUT_FILE)?;
    println!("Output: {OUTPUT_FILE}");

    // Create bounded channel for frame buffer
    let (tx, rx): (mpsc::SyncSender<CapturedFrame>, Receiver<CapturedFrame>) =
        mpsc::sync_channel(BUFFER_SIZE);

    let mut stats = PerformanceStats::new();
    let start_time = Instant::now();

    println!("\nRecording with pipelined capture/encode...");
    println!("Progress: [                                        ] 0%");

    // Start capture thread
    let capture_handle = thread::spawn(move || {
        capture_thread(&tx, width, height, RECORDING_DURATION);
    });

    // Main thread: encode loop
    let mut last_progress_print = Instant::now();

    while start_time.elapsed() < RECORDING_DURATION + Duration::from_millis(500) {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(captured) => {
                stats.total_frames += 1;
                stats.capture_times.push(captured.capture_time);

                // Encode
                let encode_start = Instant::now();
                for result in encoder.encode_nv12(&captured.nv12_data) {
                    match result {
                        Ok(data) => {
                            stats.encode_times.push(encode_start.elapsed());

                            if !data.is_empty() {
                                output_file.write_all(&data)?;
                                stats.total_bytes += data.len();
                                stats.successful_frames += 1;
                            }
                        }
                        Err(e) => {
                            eprintln!("\rEncode error: {e:?}");
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
            let progress = (elapsed.as_millis() * 100 / RECORDING_DURATION.as_millis()).min(100);
            let bar_filled =
                usize::try_from(progress / 2).expect("a progress of at most 100% fits in usize");
            let bar = "█".repeat(bar_filled);
            let remaining = " ".repeat(50 - bar_filled);
            print!(
                "\rProgress: [{bar}{remaining}] {progress}%  FPS: {:.1}  Size: {:.1}MB  ",
                f64::from(stats.successful_frames) / elapsed.as_secs_f64(),
                megabytes(stats.total_bytes)
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

    println!("\nRecording saved to: {OUTPUT_FILE}");
    println!("You can play it with: ffplay {OUTPUT_FILE}");

    Ok(())
}
