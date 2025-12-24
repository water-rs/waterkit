# waterkit-media

Cross-platform media control and audio playback for Rust.

## Features

- 🎵 **Audio Playback** - Play local files and URLs using [rodio](https://github.com/RustAudio/rodio)
- 📱 **Media Center Integration** - Display "Now Playing" info on lock screens and system UIs
- 🎮 **Media Key Support** - Respond to play/pause/seek from keyboards, headphones, and system controls
- 🌍 **Cross-Platform** - Works on macOS, iOS, Windows, Linux, and Android

| Platform | Now Playing            | Media Keys            | Audio Backend           |
| -------- | ---------------------- | --------------------- | ----------------------- |
| macOS    | MPNowPlayingInfoCenter | MPRemoteCommandCenter | rodio (CoreAudio)       |
| iOS      | MPNowPlayingInfoCenter | MPRemoteCommandCenter | rodio (CoreAudio)       |
| Windows  | SMTC                   | SMTC                  | rodio (WASAPI)          |
| Linux    | MPRIS                  | MPRIS                 | rodio (PulseAudio/ALSA) |
| Android  | MediaSession           | MediaSession          | rodio (OpenSL ES)       |

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
waterkit-media = { git = "https://github.com/water-rs/kit" }
```

## Quick Start

```rust
use waterkit_media::AudioPlayer;

#[tokio::main]
async fn main() -> Result<(), waterkit_media::PlayerError> {
    // Create a player with metadata
    let mut player = AudioPlayer::new()
        .title("Never Gonna Give You Up")
        .artist("Rick Astley")
        .album("Whenever You Need Somebody")
        .build()?;

    // Play a file
    player.play_file("song.mp3")?;

    // Handle media commands (from keyboard, headphones, etc.)
    loop {
        let cmd = player.next_command().await;
        player.handle_command(cmd);
    }
}
```

## Usage

### Basic Playback

```rust
use waterkit_media::AudioPlayer;

let mut player = AudioPlayer::new().build()?;

// Play from file
player.play_file("music.mp3")?;

// Play from URL
player.play_url("https://example.com/stream.mp3").await?;

// Control playback
player.pause();
player.resume();
player.seek(Duration::from_secs(30));
player.set_volume(0.5);
player.stop();
```

### Handling Media Commands

Media commands come from system media keys, Bluetooth headphone buttons, lock screen controls, etc.

```rust
// Async - wait for next command
let cmd = player.next_command().await;
player.handle_command(cmd);

// Non-blocking poll
if let Some(cmd) = player.try_next_command() {
    player.handle_command(cmd);
}

// Custom handling
match player.try_next_command() {
    Some(MediaCommand::Next) => play_next_track(),
    Some(MediaCommand::Previous) => play_previous_track(),
    Some(cmd) => player.handle_command(cmd),
    None => {}
}
```

### Updating Metadata

```rust
use waterkit_media::MediaMetadata;

player.set_metadata(MediaMetadata {
    title: Some("New Song".into()),
    artist: Some("Artist".into()),
    album: Some("Album".into()),
    artwork_url: Some("https://example.com/cover.jpg".into()),
    duration: Some(Duration::from_secs(180)),
});
```

### Device Selection

```rust
// List available audio devices
let devices = AudioPlayer::list_devices()?;
for device in &devices {
    println!("{}", device.name());
}

// Use a specific device
let player = AudioPlayer::new()
    .device(&devices[0])
    .build()?;
```

### Advanced: Direct Sink Access

For advanced audio manipulation, access the underlying rodio `Sink`:

```rust
let sink = player.sink();
sink.set_speed(1.5); // 1.5x playback speed
```

## Media Commands

The following commands can be received from system controls:

| Command                  | Description               |
| ------------------------ | ------------------------- |
| `Play`                   | Start/resume playback     |
| `Pause`                  | Pause playback            |
| `PlayPause`              | Toggle play/pause         |
| `Stop`                   | Stop playback             |
| `Next`                   | Skip to next track        |
| `Previous`               | Skip to previous track    |
| `Seek(Duration)`         | Seek to absolute position |
| `SeekForward(Duration)`  | Seek forward by amount    |
| `SeekBackward(Duration)` | Seek backward by amount   |

## Supported Formats

Audio format support is provided by rodio:

- MP3
- WAV
- Vorbis (OGG)
- FLAC
- AAC (platform-dependent)

## License

MIT OR Apache-2.0
