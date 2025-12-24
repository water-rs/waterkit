# Waterkit Audio

Cross-platform audio playback and recording library for WaterUI.

## Features

- **Playback**: Play audio files (MP3, WAV, AAC, etc.) with controls (Play, Pause, Stop, Seek).
- **Recording**: Record microphone input to files.
- **Volume Control**: System volume stream management.
- **Cross-Platform**: Unified API for Mobile and Desktop.

## Installation

```toml
[dependencies]
waterkit-audio = "0.1"
# OR via main crate
waterkit = { version = "0.1", features = ["audio"] }
```

## Platform Support

| Platform | Backend |
| :--- | :--- |
| **macOS/iOS** | AVFoundation / AVAudioEngine (Swift) |
| **Android** | MediaPlayer / AudioRecord (Kotlin/JNI) |
| **Windows/Linux** | `rodio`, `cpal` (Rust) |

## Usage

### Playback

```rust
use waterkit_audio::AudioPlayer;

async fn play_sound() {
    let player = AudioPlayer::new();
    
    // Load from file path (or URL on some platforms)
    player.load("assets/music.mp3").await.unwrap();
    
    player.play().await;
    
    // .. later
    player.pause().await;
}
```

### Recording

```rust
use waterkit_audio::AudioRecorder;

async fn record_voice() {
    let recorder = AudioRecorder::new();
    
    // Start recording to a specific path
    recorder.start("/tmp/voice.m4a").await.unwrap();
    
    // ... wait
    recorder.stop().await.unwrap();
}
```
