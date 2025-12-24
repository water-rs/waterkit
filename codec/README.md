# Waterkit Codec

Low-level hardware-accelerated media encoding and decoding.

## Overview

This crate provides a unified interface for accessing system media codecs. It is primarily used internally by `waterkit-video` and `waterkit-audio` but can be used for custom media processing pipelines.

## Features

- **Hardware Acceleration**: Uses specific hardware APIs where available.
- **Formats**: H.264, H.265 (HEVC), AAC.
- **Zero-Copy**: Optimized for efficient frame passing to `wgpu` textures.

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
| **Windows/Linux** | FFmpeg / Dav1d (Software fallback currently) |

## Usage

*Specific usage examples are advanced. Typically, use `waterkit-video` for playback.*

```rust
// Example: Concept of creating a decoder
use waterkit_codec::{VideoDecoder, CodecType};

let decoder = VideoDecoder::new(CodecType::H264).unwrap();
// decoder.decode(packet)...
```
