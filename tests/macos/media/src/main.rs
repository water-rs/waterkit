//! macOS test binary for waterkit-media.
//!
//! Run with: cargo run -p waterkit-media-test
//!
//! This will display "Now Playing" info in Control Center for 30 seconds.
//! Use the media controls to test command handling.

use std::sync::Arc;
use std::time::Duration;
use waterkit_media::{MediaCommand, MediaCommandHandler, MediaMetadata, MediaSession, PlaybackState};

struct TestHandler;

impl MediaCommandHandler for TestHandler {
    fn on_command(&self, command: MediaCommand) {
        println!("📱 Received command: {:?}", command);
    }
}

fn main() {
    println!("=== Waterkit Media Test (macOS) ===\n");

    // Create media session
    println!("Creating media session...");
    let session = match MediaSession::new() {
        Ok(s) => {
            println!("✓ Media session created\n");
            s
        }
        Err(e) => {
            println!("✗ Failed to create session: {}\n", e);
            return;
        }
    };

    // Request audio focus
    println!("Requesting audio focus...");
    match session.request_audio_focus() {
        Ok(()) => println!("✓ Audio focus granted\n"),
        Err(e) => println!("⚠ Audio focus issue: {}\n", e),
    }

    // Set metadata
    println!("Setting media metadata...");
    let metadata = MediaMetadata::new()
        .title("Test Song")
        .artist("Waterkit Test Artist")
        .album("Test Album")
        .duration(Duration::from_secs(180));

    match session.set_metadata(&metadata) {
        Ok(()) => {
            println!("✓ Metadata set:");
            println!("  Title:    {}", metadata.title.as_deref().unwrap_or("-"));
            println!("  Artist:   {}", metadata.artist.as_deref().unwrap_or("-"));
            println!("  Album:    {}", metadata.album.as_deref().unwrap_or("-"));
            println!("  Duration: {:?}\n", metadata.duration);
        }
        Err(e) => println!("✗ Failed to set metadata: {}\n", e),
    }

    // Set playback state
    println!("Setting playback state to Playing...");
    match session.set_playback_state(&PlaybackState::playing(Duration::from_secs(30))) {
        Ok(()) => println!("✓ Playback state: Playing at 0:30\n"),
        Err(e) => println!("✗ Failed to set state: {}\n", e),
    }

    // Register command handler
    println!("Registering command handler...");
    match session.set_command_handler(TestHandler) {
        Ok(()) => println!("✓ Command handler registered\n"),
        Err(e) => println!("⚠ Command handler issue: {}\n", e),
    }

    println!("========================================");
    println!("Check Control Center (top-right menu bar)");
    println!("for 'Now Playing' information!");
    println!("");
    println!("Try using media keys or Control Center");
    println!("to send commands. This test will run");
    println!("for 30 seconds...");
    println!("========================================\n");

    // Keep running for 30 seconds to allow testing
    std::thread::sleep(Duration::from_secs(30));

    // Clean up
    println!("\nCleaning up...");
    match session.abandon_audio_focus() {
        Ok(()) => println!("✓ Audio focus abandoned"),
        Err(e) => println!("⚠ {}", e),
    }
    match session.clear() {
        Ok(()) => println!("✓ Session cleared"),
        Err(e) => println!("⚠ {}", e),
    }

    println!("\n=== Test Complete ===");
}
