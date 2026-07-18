# WaterKit Video

`waterkit-video` is the UI-independent umbrella for WaterKit's modular video
stack. It exposes media timing and color metadata, containers, Zenwave-based
delivery, typed frame processing, platform codecs, playback, DRM integration,
and platform media services without depending on WaterUI.

The umbrella keeps each reusable compilation boundary available as its own
crate:

- `waterkit-video-core`: media timing, color, HDR, and protection types.
- `waterkit-video-container`: MP4/MOV, progressive tracks, CMAF, MPEG-TS,
  subtitles, timed metadata, muxing, and demuxing.
- `waterkit-video-streaming`: progressive download, HLS, LL-HLS, DASH,
  LL-DASH, ABR, caching, offline packages, and license transport. Every network
  request goes through Zenwave.
- `waterkit-video-processing`: typed asynchronous frame-processing pipelines,
  with optional Filtrate GPU effects.
- `waterkit-video-player`: decode, playback timing, tracks, live playback,
  picture in picture, media sessions, and platform DRM/output integration.
- `waterkit-video-cli`: media inspection and reproducible decode benchmarks.

## Feature selection

```toml
[dependencies]
waterkit-video = { version = "0.1", default-features = false, features = ["container"] }
```

Available umbrella features:

- `container`: container readers, writers, subtitles, and metadata.
- `player`: container support plus the playback engine. This is the default.
- `streaming`: Zenwave delivery, HLS, DASH, ABR, and streaming playback.
- `offline`: persistent media packages and offline download management.
- `processing`: typed frame processing.
- `filtrate`: Filtrate-backed GPU processing.
- `full`: every layer above.

Selecting only `container` does not pull in Zenwave, audio output, GPU
processing, or the player. Selecting no features keeps only
`waterkit-video-core`.

## Container example

```rust
use waterkit_video::{VideoError, VideoReader};

fn inspect(path: &str) -> Result<(), VideoError> {
    let reader = VideoReader::open(path)?;
    let (width, height) = reader.dimensions();
    assert!(width > 0 && height > 0);
    Ok(())
}
```

WaterUI's semantic video API lives in `waterui-video`, and its portable
self-drawn controls and GPU presentation live in `waterui-video-gpu`. Neither
layer is required to reuse WaterKit Video in another Rust application.

## Playback capability boundary

Media3/ExoPlayer is used as a comparison baseline, not as a dependency. The
current WaterKit player implements the following reusable playback core:

| Area | Implemented scope |
| --- | --- |
| Playback model | playlists, stable item identity, previous/next, repeat, shuffle, seeking, frame stepping, playback rate, pitch preservation, volume, mute, and events |
| Delivery | progressive byte-range loading, HLS, LL-HLS, DASH, LL-DASH, CDN failover, ABR, live seek windows, catch-up, cache revalidation, and resumable offline packages through Zenwave |
| Containers | progressive MP4/MOV, fragmented MP4/CMAF, and MPEG-TS streaming |
| Video | H.264, H.265/HEVC, and AV1 decode with NV12/P010 GPU presentation, HDR color metadata, tone mapping, and spherical video projection |
| Audio | cross-platform AAC decode; Android compressed offload for AAC, AC-3, E-AC-3, AC-4, Opus, and FLAC when the device accepts the format |
| Tracks and metadata | adaptive audio/video track selection, WebVTT, TTML/IMSC, SubRip sidecars, ID3, and `emsg` |
| Protected playback | CENC/CBCS parsing, Android `MediaDrm` online/offline license lifecycles, secure surfaces, and protected decoder output |
| Platform integration | media sessions, audio focus, output-device selection, picture in picture, Android audio offload, and Android A/V tunneling |
| Observability | startup time, buffered duration, rebuffer count/duration, dropped frames, A/V drift, throughput, selected output path, and a reproducible decode benchmark CLI |

The following Media3/ExoPlayer areas are explicitly not implemented today:

- SmoothStreaming and RTSP delivery.
- WebM, Matroska, Ogg, MP3, ADTS, AMR, FLV, and other progressive container
  extractors outside the MP4/MOV path.
- General cross-platform decode for VP8/VP9, MPEG-2, Vorbis, Opus, FLAC,
  Dolby, DTS, and MPEG-H tracks. Some compressed audio formats remain available
  only through Android hardware offload.
- CEA-608/708, SSA/ASS, DVB, and PGS subtitle decoding.
- ICY metadata and CMCD request instrumentation.
- Media3-style declarative track constraints such as preferred languages and
  roles, maximum dimensions/bitrate, and audio channel-count constraints;
  WaterKit currently exposes automatic ABR or stable explicit track selection.
- A persistent Android playback/download service, notification ownership,
  background download scheduling constraints, and `DefaultPreloadManager`-style
  adjacent-item orchestration. Those are host-application services built above
  the reusable player and offline package APIs.
- Cast route playback and protected playback schemes outside the implemented
  Android `MediaDrm` path. Advertising is likewise an optional product
  integration rather than a responsibility of the core player timeline.

Unsupported capabilities fail explicitly; they do not silently switch to a
different engine or pull Media3/ExoPlayer into the application dependency graph.
