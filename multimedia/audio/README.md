# WaterKit Audio

Cross-platform audio playback, packet decoding, streaming PCM output,
recording, media-session integration, playback-rate control, skip-silence,
spatial playback, and explicit output-device selection.

## Features

The default feature set enables the complete audio stack. Consumers can disable
defaults and select only the layers they use:

- `playback`: file and Zenwave URL playback.
- `recording`: microphone capture.
- `streaming`: packet AAC decode and incremental PCM output for media engines.
- `he-aac`: complete AAC-LC, HE-AAC, and HE-AAC v2 decoding where FDK is used.
- `media-session`: platform Now Playing, transport controls, and audio focus.
- `apple-artwork`: Zenwave artwork loading for Apple media sessions.

```toml
[dependencies]
waterkit-audio = { version = "0.1", default-features = false, features = ["playback"] }
```

## Playback

```rust
use waterkit_audio::{AudioPlayer, PlayerError};

fn play(path: &str) -> Result<AudioPlayer, PlayerError> {
    let player = AudioPlayer::open(path)?;
    player.play();
    Ok(player)
}
```

`AudioPlayer::open_url` performs remote requests through Zenwave. Output
devices come from `AudioDevice::list`, and `AudioOutput::on_device` pins one
player to the selected device. On iOS, output routing remains system-owned and
explicit device selection returns an error.

## Recording

```rust
use futures::StreamExt as _;
use waterkit_audio::{AudioRecorder, RecordError};

async fn capture_one_buffer() -> Result<(), RecordError> {
    let mut recorder = AudioRecorder::new()
        .sample_rate(48_000)
        .channels(1)
        .build()?;
    recorder.start().await?;
    let stream = recorder.stream();
    futures::pin_mut!(stream);
    let _buffer = stream.next().await;
    recorder.stop().await
}
```

Platform media services are native integrations: AVFoundation and Now Playing
on iOS, Android media sessions and audio focus, Windows system media transport
controls, and Linux MPRIS. Decoded PCM output uses each platform's audio device
backend rather than a WaterUI dependency.
