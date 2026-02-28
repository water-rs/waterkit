# Waterkit Codec

Low-level hardware-accelerated media encoding and decoding.

## Overview

This crate provides a unified interface for accessing system media codecs. It is primarily used internally by `waterkit-video` and `waterkit-audio` but can be used for custom media processing pipelines.

## Features

- **Hardware Acceleration**: Uses platform hardware APIs where available.
- **Formats**: H.264, H.265 (HEVC), AV1.
- **Zero-Copy**: Optimized for efficient frame passing to `wgpu` textures.
- **Software Fallback (optional)**: AV1 software fallback is controlled by `software-fallback` feature (enabled by default).
- **Silent AV1 HDR Fallback**: When AV1 input is HDR but hardware HDR decode is unsupported, codec silently prefers software AV1; if software AV1 is unavailable, it silently falls back to SDR hardware decode.

## Installation

```toml
[dependencies]
waterkit-codec = "0.1"
```

## Platform Support

| Platform | Technology |
| :--- | :--- |
| **macOS/iOS** | VideoToolbox |
| **Android** | MediaCodec (NDK/JNI) |
| **Windows** | Media Foundation (hardware MFT) |
| **Linux** | VA-API (`libva`, DRM render node) |

## Usage

*Specific usage examples are advanced. Typically, use `waterkit-video` for playback.*

```rust
// Example: Concept of creating a decoder
use waterkit_codec::{VideoDecoder, CodecType};

let decoder = VideoDecoder::new(CodecType::H264).unwrap();
// decoder.decode(packet)...
```
