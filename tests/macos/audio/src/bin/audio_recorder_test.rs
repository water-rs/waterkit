//! macOS test binary for waterkit-audio recorder.
//!
//! Run with: cargo run -p waterkit-audio-test --bin audio-recorder-test

use futures::StreamExt;
use std::time::Duration;
use waterkit_audio::AudioRecorder;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Waterkit AudioRecorder Async Test ===\n");

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
        tokio::pin!(stream);
        
        let mut packet_count = 0;
        let mut total_samples = 0;

        // Use a timeout to stop test
        let timeout = tokio::time::sleep(Duration::from_secs(3));
        tokio::pin!(timeout);

        loop {
            tokio::select! {
                Some(buffer) = stream.next() => {
                    packet_count += 1;
                    total_samples += buffer.len();
                    if packet_count % 10 == 0 {
                        print!(".");
                        use std::io::Write;
                        std::io::stdout().flush()?;
                    }
                }
                _ = &mut timeout => {
                    println!("\nTime's up!");
                    break;
                }
            }
        }
        println!("\nCaptured {} packets, {} total samples", packet_count, total_samples);
        println!("Average packet size: {:.1} samples", total_samples as f64 / if packet_count > 0 { packet_count as f64 } else { 1.0 });

        if packet_count == 0 {
             return Err("No audio data received".into());
        }
    }

    // 4. Stop Recording
    println!("Stopping recording...");
    recorder.stop().await?;
    println!("✓ Recording stopped");

    println!("\n=== Test PASSED ===");
    Ok(())
}
