//! macOS test binary for waterkit-audio recorder.
//!
//! Run with: cargo run -p waterkit-audio-test --bin audio-recorder-test

use futures::{FutureExt, StreamExt};
use std::io::Write;
use std::time::Duration;
use waterkit_audio::AudioRecorder;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Waterkit AudioRecorder Async Test ===\n");

    futures::executor::block_on(async {
        // 1. Initialize Recorder
        println!("Initializing recorder...");
        let mut recorder = AudioRecorder::new()
            .sample_rate(44100)
            .channels(1)
            .build()?;
        println!("✓ Recorder initialized");

        // 2. Start Recording
        println!("Starting recording...");
        recorder.start().await?;
        println!("✓ Recording started");

        // 3. Consume Stream
        println!("Capturing audio for 3 seconds...");
        {
            let stream = recorder.stream();
            futures::pin_mut!(stream);

            let mut packet_count: usize = 0;
            let mut total_samples: usize = 0;
            let start = std::time::Instant::now();

            loop {
                let mut next_packet = stream.next().fuse();
                let mut timeout = futures_timer::Delay::new(
                    Duration::from_secs(3).saturating_sub(start.elapsed()),
                )
                .fuse();

                if start.elapsed() >= Duration::from_secs(3) {
                    println!("\nTime's up!");
                    break;
                }

                futures::select! {
                     packet = next_packet => {
                        if let Some(buffer) = packet {
                            packet_count += 1;
                            total_samples += buffer.len();
                            if packet_count.is_multiple_of(10) {
                                print!(".");
                                let _ = std::io::stdout().flush();
                            }
                        } else {
                            break; // Stream ended
                        }
                     },
                     () = timeout => {
                         println!("\nTime's up!");
                         break;
                     }
                }
            }
            println!("\nCaptured {packet_count} packets, {total_samples} total samples");
            if packet_count == 0 {
                return Err("No audio data received".into());
            }
            println!(
                "Average packet size: {} samples",
                total_samples / packet_count
            );
        }

        // 4. Stop Recording
        println!("Stopping recording...");
        recorder.stop().await?;
        println!("✓ Recording stopped");

        println!("\n=== Test PASSED ===");
        Ok(())
    })
}
