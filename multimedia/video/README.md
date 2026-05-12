# Waterkit Video

Cross-platform video playback and processing.

## Features

- **Playback**: Play video files with hardware acceleration.
- **Muxing**: Create MP4/MOV files.
- **WGPU Integration**: Render video frames directly to `wgpu` textures.

## Installation

```toml
[dependencies]
waterkit-video = "0.1"
```

## Backend

This crate sits on top of `waterkit-codec` to provide higher-level features.

- **Encoding/Decoding**: Uses platform hardware codecs via `waterkit-codec`.
- **Container**: Uses `mp4` crate for container parsing/writing.

## Usage

```rust
// Streaming example (Conceptual)
use waterkit_video::VideoPlayer;

async fn play_video() {
    let player = VideoPlayer::new();
    player.load("assets/video.mp4").await.unwrap();
    
    // In your render loop
    let frame_texture = player.get_current_frame().await;
    // render frame_texture with wgpu...
}
```
