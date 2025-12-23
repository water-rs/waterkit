//! macOS test binary for waterkit-media.
//!
//! Run with: cargo run -p waterkit-media-test [options] [audio_file]
//!
//! Options:
//!   --title <title>      Set the track title
//!   --artist <artist>    Set the artist name
//!   --album <album>      Set the album name
//!   --artwork <path>     Set artwork image path
//!
//! Examples:
//!   cargo run -p waterkit-media-test
//!   cargo run -p waterkit-media-test /tmp/song.mp3
//!   cargo run -p waterkit-media-test --title "My Song" --artist "Artist" /tmp/song.mp3

use std::time::Duration;
use waterkit_media::{AudioPlayer, MediaCommand};

fn parse_args() -> (Option<String>, Option<String>, Option<String>, Option<String>, Option<String>) {
    let args: Vec<String> = std::env::args().collect();
    let mut title = None;
    let mut artist = None;
    let mut album = None;
    let mut artwork = None;
    let mut audio_file = None;
    
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--title" if i + 1 < args.len() => {
                title = Some(args[i + 1].clone());
                i += 2;
            }
            "--artist" if i + 1 < args.len() => {
                artist = Some(args[i + 1].clone());
                i += 2;
            }
            "--album" if i + 1 < args.len() => {
                album = Some(args[i + 1].clone());
                i += 2;
            }
            "--artwork" if i + 1 < args.len() => {
                artwork = Some(args[i + 1].clone());
                i += 2;
            }
            arg if !arg.starts_with("--") => {
                audio_file = Some(arg.to_string());
                i += 1;
            }
            _ => i += 1,
        }
    }
    
    (audio_file, title, artist, album, artwork)
}

fn expand_path(path: &str) -> String {
    if path.starts_with("~/") {
        let home = std::env::var("HOME").unwrap_or_default();
        path.replacen("~", &home, 1)
    } else {
        path.to_string()
    }
}

#[tokio::main]
async fn main() {
    let (audio_file, title, artist, album, artwork) = parse_args();
    
    println!("=== Waterkit Media AudioPlayer Test (macOS) ===\n");

    // List available devices
    println!("Available audio devices:");
    match AudioPlayer::list_devices() {
        Ok(devices) => {
            for (i, device) in devices.iter().enumerate() {
                println!("  [{}] {}", i, device.name());
            }
        }
        Err(e) => println!("  (error: {})", e),
    }
    println!();

    // Determine metadata
    let track_title = title.unwrap_or_else(|| {
        audio_file.as_ref()
            .map(|f| std::path::Path::new(f).file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Test Audio")
                .to_string())
            .unwrap_or_else(|| "Test Audio".to_string())
    });
    let track_artist = artist.unwrap_or_else(|| "Unknown Artist".to_string());
    let track_album = album.unwrap_or_else(|| "Unknown Album".to_string());
    
    // Create audio player with metadata
    println!("Creating audio player...");
    let mut builder = AudioPlayer::new()
        .title(&track_title)
        .artist(&track_artist)
        .album(&track_album);
    
    // Add artwork if provided (convert local path to file:// URL)
    if let Some(art_path) = artwork {
        let expanded = expand_path(&art_path);
        let artwork_url = format!("file://{}", expanded);
        builder = builder.artwork_url(&artwork_url);
        println!("Artwork: {}", expanded);
    }
    
    let player = match builder.build() {
        Ok(p) => {
            println!("✓ Audio player created\n");
            p
        }
        Err(e) => {
            println!("✗ Failed to create player: {}\n", e);
            return;
        }
    };

    // Print metadata
    println!("Now Playing:");
    println!("  Title:  {}", track_title);
    println!("  Artist: {}", track_artist);
    println!("  Album:  {}", track_album);
    println!();

    // Play audio
    if let Some(file_path) = audio_file {
        let expanded_path = expand_path(&file_path);
        
        println!("Playing: {}", expanded_path);
        match player.play_file(&expanded_path) {
            Ok(()) => println!("✓ Audio playback started\n"),
            Err(e) => {
                println!("✗ Failed to play file: {}\n", e);
                return;
            }
        }
    } else {
        // Default: play a sine wave test tone
        println!("No file specified, playing 440Hz test tone...");
        {
            use waterkit_media::rodio::source::{SineWave, Source};
            let source = SineWave::new(440.0)
                .take_duration(Duration::from_secs(5))
                .amplify(0.3);
            player.sink().append(source);
        }
        println!("✓ Test tone playing\n");
    }

    // Register command handler
    player.set_command_handler(|cmd: MediaCommand| {
        match cmd {
            MediaCommand::Play => println!("📱 Play"),
            MediaCommand::Pause => println!("📱 Pause"),
            MediaCommand::PlayPause => println!("📱 Play/Pause"),
            MediaCommand::Stop => println!("📱 Stop"),
            _ => println!("📱 {:?}", cmd),
        }
    });

    println!("Playing: {}", player.is_playing());
    println!("Volume: {:.0}%", player.volume() * 100.0);
    println!();
    println!("Press Ctrl+C to stop...\n");

    // Wait until audio finishes or 10 minutes max
    player.run_loop(Duration::from_secs(600));

    player.stop();
    println!("\n=== Playback Complete ===");
}
