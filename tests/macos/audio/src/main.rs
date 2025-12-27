//! macOS test binary for waterkit-audio.
//!
//! Run with: `cargo run -p waterkit-audio-test [options] [audio_file]`
//!
//! Options:
//!   `--title <title>`      Set the track title
//!   `--artist <artist>`    Set the artist name
//!   `--album <album>`      Set the album name
//!   `--artwork <path>`     Set artwork image path

use std::time::Duration;
use waterkit_audio::AudioPlayer;

struct Args {
    audio_file: Option<String>,
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    artwork: Option<String>,
}

fn parse_args() -> Args {
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

    Args {
        audio_file,
        title,
        artist,
        album,
        artwork,
    }
}

fn expand_path(path: &str) -> String {
    if path.starts_with("~/") {
        let home = std::env::var("HOME").unwrap_or_default();
        path.replacen('~', &home, 1)
    } else {
        path.to_string()
    }
}

fn main() {
    let args = parse_args();

    println!("=== Waterkit Media AudioPlayer Test (macOS) ===\n");

    // Determine metadata
    let track_title = args.title.unwrap_or_else(|| {
        args.audio_file
            .as_ref()
            .map(|f| {
                std::path::Path::new(f)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Test Audio")
                    .to_string()
            })
            .unwrap_or_else(|| "Test Audio".to_string())
    });
    let track_artist = args.artist.unwrap_or_else(|| "Unknown Artist".to_string());
    let track_album = args.album.unwrap_or_else(|| "Unknown Album".to_string());

    // Create audio player with metadata
    println!("Creating audio player...");
    let mut builder = AudioPlayer::new()
        .title(&track_title)
        .artist(&track_artist)
        .album(&track_album);

    // Add artwork if provided (convert local path to file:// URL)
    if let Some(art_path) = args.artwork {
        let expanded = expand_path(&art_path);
        let artwork_url = format!("file://{}", expanded);
        builder = builder.artwork_url(&artwork_url);
        println!("Artwork: {}", expanded);
    }

    let mut player = match builder.build() {
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
    if let Some(file_path) = args.audio_file {
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
            use waterkit_audio::rodio::source::{SineWave, Source};
            let source = SineWave::new(440.0)
                .take_duration(Duration::from_secs(30))
                .amplify(0.3);
            player.sink().append(source);
        }
        println!("✓ Test tone playing\n");
    }

    // Use default command handler which handles Play, Pause, Stop, Seek, etc. automatically
    player.set_default_handler();

    println!("Controls:");
    println!("  - Use media keys or Control Center to pause/play/seek");
    println!("  - Press Ctrl+C to stop");
    println!();

    // Main loop - handle commands and keep playing
    while !player.sink().empty() || player.sink().is_paused() {
        // Check if stopped manually (via command)
        if player.state() == waterkit_audio::PlayerState::Stopped {
            break;
        }

        // Update progress bar periodically
        player.update_now_playing();

        player.run_loop(Duration::from_millis(500));
    }

    // RAII Drop will clear the media session
    println!("\n=== Playback Complete ===");
}
