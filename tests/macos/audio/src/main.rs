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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args();

    println!("=== Waterkit Media AudioPlayer Test (macOS) ===\n");

    let player = if let Some(file_path) = args.audio_file {
        let expanded_path = expand_path(&file_path);
        println!("Opening: {}", expanded_path);

        let p = AudioPlayer::open(&expanded_path)?;
        println!("✓ Audio opened");

        // Apply overrides
        let mut p = p;
        if let Some(t) = args.title {
            p = p.title(t);
        }
        if let Some(a) = args.artist {
            p = p.artist(a);
        }
        if let Some(a) = args.album {
            p = p.album(a);
        }
        if let Some(art) = args.artwork {
            let expanded = expand_path(&art);
            let url = format!("file://{}", expanded);
            p = p.artwork_url(url);
            println!("Artwork: {}", expanded);
        }

        println!("\nNow Playing (Metadata):");
        println!("  Title:  {:?}", p.metadata().title);
        println!("  Artist: {:?}", p.metadata().artist);
        println!("  Album:  {:?}", p.metadata().album);
        println!();

        println!("Starting playback...");
        p.play();
        p
    } else {
        println!("No file specified. Usage: cargo run -p waterkit-audio-test -- <file> [options]");
        return Ok(());
    };

    println!("Controls:");
    println!("  - Use media keys or Control Center to pause/play/seek");
    println!("  - Press Ctrl+C to stop");
    println!();

    // Commands channel
    let commands = player.commands();
    // We need to poll commands. Since we are in sync main, we can stick to a simple loop
    // that sleeps and occasionally polls if we had a blocking iterator,
    // but commands() returns a Stream.

    // We can just sleep and let the background thread handle everything,
    // AS LONG AS we don't need to do custom handling in this main thread.
    // The player background thread handles polling commands and putting them in the queue.
    // But SOMEONE needs to read the queue and call handle().

    // Since we are in a sync main, let's spawn a thread to handle commands using block_on
    // or just run a loop here.

    let player_ref = &player;
    std::thread::scope(|s| {
        s.spawn(move || {
            // Simple blocking loop to print status
            loop {
                if !player_ref.is_playing()
                    && player_ref.position().as_secs() > 0
                    && player_ref
                        .metadata()
                        .duration
                        .map_or(false, |d| player_ref.position() >= d)
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1000));
            }
        });

        // Run async command handler on main thread
        futures::executor::block_on(async {
            use futures::StreamExt;
            let commands = commands; // move into async block
            futures::pin_mut!(commands);

            while let Some(cmd) = commands.next().await {
                println!("Received command: {:?}", cmd);
                player_ref.handle(&cmd);

                if matches!(cmd, waterkit_audio::MediaCommand::Stop) {
                    break;
                }
            }
        });
    });

    println!("\n=== Playback Complete ===");
    Ok(())
}
