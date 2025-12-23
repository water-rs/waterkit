//! macOS test binary for waterkit-media.
//!
//! Run with: cargo run -p waterkit-media-test
//!
//! This will play audio and show "Now Playing" info in Control Center.
//! Use the media controls to test command handling.

use std::time::Duration;
use waterkit_media::{AudioPlayer, MediaCommand};

fn main() {
    println!("=== Waterkit Media AudioPlayer Test (macOS) ===\n");

    // Create audio player with metadata
    println!("Creating audio player...");
    let player = match AudioPlayer::new()
        .title("Test Audio")
        .artist("Waterkit Test")
        .album("Test Album")
        .build()
    {
        Ok(p) => {
            println!("✓ Audio player created\n");
            p
        }
        Err(e) => {
            println!("✗ Failed to create player: {}\n", e);
            return;
        }
    };

    // Try to play a test audio URL (public domain music)
    // This is a short audio sample from the Internet Archive
    let test_url = "https://upload.wikimedia.org/wikipedia/commons/c/c8/Example.ogg";
    
    println!("Playing test audio from URL...");
    println!("URL: {}", test_url);
    match player.play_url(test_url) {
        Ok(()) => println!("✓ Audio playback started\n"),
        Err(e) => {
            println!("✗ Failed to play audio: {}\n", e);
            println!("Note: Audio playback is required for Now Playing to work on macOS.");
            println!("The test will continue but Now Playing may not appear.\n");
        }
    }

    // Register command handler
    println!("Registering command handler...");
    player.set_command_handler(|cmd: MediaCommand| {
        match cmd {
            MediaCommand::Play => {
                println!("📱 Command: Play");
            }
            MediaCommand::Pause => {
                println!("📱 Command: Pause");
            }
            MediaCommand::PlayPause => {
                println!("📱 Command: Play/Pause");
            }
            MediaCommand::Stop => {
                println!("📱 Command: Stop");
            }
            MediaCommand::Next => {
                println!("📱 Command: Next");
            }
            MediaCommand::Previous => {
                println!("📱 Command: Previous");
            }
            MediaCommand::Seek(pos) => {
                println!("📱 Command: Seek to {:?}", pos);
            }
            _ => {
                println!("📱 Command: {:?}", cmd);
            }
        }
    });
    println!("✓ Command handler registered\n");

    // Print current state
    if let Some(duration) = player.duration() {
        println!("Audio duration: {:?}", duration);
    }
    println!("Playing: {}", player.is_playing());
    println!("");

    println!("========================================");
    println!("Check Control Center (top-right menu bar)");
    println!("for 'Now Playing' information!");
    println!("");
    println!("Try using media keys or Control Center");
    println!("to send commands. This test will run");
    println!("for 30 seconds...");
    println!("========================================\n");

    // Run event loop for 30 seconds
    player.run_loop(Duration::from_secs(30));

    // Clean up
    println!("\nCleaning up...");
    match player.stop() {
        Ok(()) => println!("✓ Playback stopped"),
        Err(e) => println!("⚠ {}", e),
    }

    println!("\n=== Test Complete ===");
}

