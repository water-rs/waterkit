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

### Cargo features

| Feature | Default | Gates |
| :--- | :--- | :--- |
| `gpu` | yes | `wgpu` texture upload and the YUV-to-linear-RGBA compute conversion (`GpuFrame`, `DecodedFrameUploader`, `LinearRgbaConverter`, `DecodedFrame::to_gpu_frame`), plus the `wgpu`, `wgpu-hal` and `shaderloom` dependencies. |
| `software-fallback` | yes | AV1 encode and decode in software on desktop platforms (`rav1e`, `rav1d`, `avif-parse`, `yuv`, `moxcms`). |

A consumer that only wants decoded pixels can take
`default-features = false, features = ["software-fallback"]` and link no `wgpu`
at all; decoded planes come out through `DecodedFrame::copy_to_buffer` and
`Decoder::decode_into`.

## Platform Support

| Platform | Technology |
| :--- | :--- |
| **macOS/iOS** | VideoToolbox |
| **Android** | MediaCodec (NDK/JNI) |
| **Windows** | Media Foundation |
| **Linux** | VA-API / rav1d |

## Usage

*Specific usage examples are advanced. Typically, use `waterkit-video` for playback.*

```rust
// Example: Concept of creating a decoder
use waterkit_codec::{VideoDecoder, CodecType};

let decoder = VideoDecoder::new(CodecType::H264).unwrap();
// decoder.decode(packet)...
```
