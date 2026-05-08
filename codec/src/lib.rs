//! GPU-first zero-copy video codec with streaming API.
//!
//! This crate provides hardware-accelerated video encoding and decoding with
//! lazy GPU texture creation. Decoded frames are returned as an iterator of
//! opaque [`DecodedFrame`] types that can be converted to GPU textures when needed.
//!
//! # Example
//!
//! ```ignore
//! // Create decoder without wgpu device
//! let mut decoder = Decoder::new(CodecType::H265, config, 1920, 1080)?;
//!
//! // Decode returns a streaming iterator (no GPU allocation yet)
//! for frame in decoder.decode(compressed_data) {
//!     let frame = frame?;
//!     let gpu_frame = frame.to_gpu_frame(&my_device, &my_queue);
//!
//!     // Use YUV textures directly in shader
//!     let y = gpu_frame.y_texture();
//!     let uv = gpu_frame.uv_texture();
//!
//!     // Or convert to RGBA on GPU
//!     let rgba = gpu_frame.to_rgba(&my_device, &my_queue);
//! }
//! ```
//!
//! # Mapped Buffer Path (Zero-Copy)
//!
//! For true zero-copy on unified memory systems (Apple Silicon, integrated GPUs),
//! use [`Decoder::decode_into`] to decode directly into a mapped GPU buffer:
//!
//! ```ignore
//! let buffer = device.create_buffer(&BufferDescriptor {
//!     size: frame_size,
//!     usage: BufferUsages::MAP_WRITE | BufferUsages::COPY_SRC,
//!     mapped_at_creation: true,
//! });
//!
//! let slice = buffer.slice(..).get_mapped_range_mut();
//! for info in decoder.decode_into(data, &mut slice) {
//!     let info = info?;
//!     // Process frame info...
//! }
//! drop(slice);
//! buffer.unmap();
//! ```

#![warn(missing_docs)]

mod frame;
mod image;
#[cfg(target_vendor = "apple")]
mod image_apple;
mod software;
mod sys;

pub use frame::{DecodedFrame, GpuFrame, YuvConverter};
pub use image::{DecodedImage, DecodedPixelFormat, decode_image, decode_image_platform};

use std::vec::IntoIter;
use thiserror::Error;

/// Codec error type.
#[derive(Debug, Clone, Error)]
pub enum CodecError {
    /// The codec or format is not supported.
    #[error("unsupported codec or format: {0}")]
    Unsupported(String),
    /// Initialization failed.
    #[error("initialization failed: {0}")]
    InitializationFailed(String),
    /// Encoding failed.
    #[error("encoding failed: {0}")]
    EncodingFailed(String),
    /// Decoding failed.
    #[error("decoding failed: {0}")]
    DecodingFailed(String),
}

/// Supported codec types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodecType {
    /// H.264 (AVC) - hardware only
    H264,
    /// H.265 (HEVC) - hardware only
    H265,
    /// AV1 - software fallback available
    Av1,
}

/// Frame info returned when decoding into a mapped buffer.
///
/// Use this with [`Decoder::decode_into`] for zero-copy on unified memory systems.
#[derive(Debug, Clone, Copy)]
pub struct FrameInfo {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Presentation timestamp.
    pub timestamp: std::time::Duration,
    /// Offset in buffer for Y plane data.
    pub y_offset: usize,
    /// Offset in buffer for UV plane data.
    pub uv_offset: usize,
    /// Total bytes used for this frame (Y + UV).
    pub total_bytes: usize,
}

/// Streaming iterator over decoded frames.
///
/// Created by [`Decoder::decode`]. Yields frames one at a time.
pub struct DecodeStream {
    inner: DecodeStreamInner,
}

enum DecodeStreamInner {
    /// Successful decode - yields frames from the iterator.
    Frames(IntoIter<DecodedFrame>),
    /// Decode error - yields the error once, then empty.
    Error(Option<CodecError>),
}

impl Iterator for DecodeStream {
    type Item = Result<DecodedFrame, CodecError>;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            DecodeStreamInner::Frames(iter) => iter.next().map(Ok),
            DecodeStreamInner::Error(err) => err.take().map(Err),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match &self.inner {
            DecodeStreamInner::Frames(iter) => iter.size_hint(),
            DecodeStreamInner::Error(Some(_)) => (1, Some(1)),
            DecodeStreamInner::Error(None) => (0, Some(0)),
        }
    }
}

impl std::fmt::Debug for DecodeStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecodeStream").finish_non_exhaustive()
    }
}

/// Streaming iterator over frame info when decoding into a buffer.
///
/// Created by [`Decoder::decode_into`]. Yields frame info one at a time.
pub struct DecodeIntoStream {
    inner: DecodeIntoStreamInner,
}

enum DecodeIntoStreamInner {
    /// Successful decode - yields frame infos from the iterator.
    Infos(IntoIter<FrameInfo>),
    /// Decode error - yields the error once, then empty.
    Error(Option<CodecError>),
}

impl Iterator for DecodeIntoStream {
    type Item = Result<FrameInfo, CodecError>;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            DecodeIntoStreamInner::Infos(iter) => iter.next().map(Ok),
            DecodeIntoStreamInner::Error(err) => err.take().map(Err),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match &self.inner {
            DecodeIntoStreamInner::Infos(iter) => iter.size_hint(),
            DecodeIntoStreamInner::Error(Some(_)) => (1, Some(1)),
            DecodeIntoStreamInner::Error(None) => (0, Some(0)),
        }
    }
}

impl std::fmt::Debug for DecodeIntoStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecodeIntoStream").finish_non_exhaustive()
    }
}

/// Streaming iterator over encoded packets.
///
/// Created by [`Encoder::encode`]. Yields encoded data packets one at a time.
pub struct EncodeStream {
    inner: EncodeStreamInner,
}

enum EncodeStreamInner {
    /// Successful encode - yields packets from the iterator.
    Packets(IntoIter<Vec<u8>>),
    /// Encode error - yields the error once, then empty.
    Error(Option<CodecError>),
}

impl Iterator for EncodeStream {
    type Item = Result<Vec<u8>, CodecError>;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            EncodeStreamInner::Packets(iter) => iter.next().map(Ok),
            EncodeStreamInner::Error(err) => err.take().map(Err),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match &self.inner {
            EncodeStreamInner::Packets(iter) => iter.size_hint(),
            EncodeStreamInner::Error(Some(_)) => (1, Some(1)),
            EncodeStreamInner::Error(None) => (0, Some(0)),
        }
    }
}

impl std::fmt::Debug for EncodeStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncodeStream").finish_non_exhaustive()
    }
}

/// Unified video decoder with automatic hardware/software selection.
///
/// Tries hardware acceleration first, falls back to software if unavailable.
/// No GPU device is required until you convert frames with [`DecodedFrame::to_gpu_frame`].
pub struct Decoder {
    inner: DecoderInner,
}

enum DecoderInner {
    #[cfg(target_vendor = "apple")]
    Apple(sys::apple::AppleDecoder),
    #[cfg(target_os = "android")]
    Android(sys::android::AndroidDecoder),
    #[cfg(target_os = "windows")]
    Windows(sys::windows::WindowsDecoder),
    #[cfg(target_os = "linux")]
    Linux(sys::linux::LinuxDecoder),
    #[cfg(all(
        feature = "software-fallback",
        not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
    ))]
    Av1(software::av1::Av1Decoder),
}

impl std::fmt::Debug for Decoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decoder").finish_non_exhaustive()
    }
}

impl Decoder {
    /// Create a new decoder.
    ///
    /// For H.264/H.265, `config` should be the codec configuration (avcC/hvcC atom).
    /// For AV1, `config` can be `None`.
    ///
    /// No GPU device is required - decoded frames can be converted to GPU textures
    /// later using [`DecodedFrame::to_gpu_frame`].
    ///
    /// # Errors
    ///
    /// Returns error if no suitable decoder is available.
    pub fn new(
        codec: CodecType,
        config: Option<&[u8]>,
        width: u32,
        height: u32,
    ) -> Result<Self, CodecError> {
        let inner = match codec {
            #[cfg(target_vendor = "apple")]
            CodecType::H264 | CodecType::H265 => {
                let apple_codec = match codec {
                    CodecType::H264 => sys::apple::CodecType::H264,
                    CodecType::H265 => sys::apple::CodecType::H265,
                    CodecType::Av1 => unreachable!(),
                };
                DecoderInner::Apple(sys::apple::AppleDecoder::new(
                    apple_codec,
                    config,
                    width,
                    height,
                )?)
            }

            #[cfg(target_os = "android")]
            CodecType::H264 | CodecType::H265 => {
                let android_codec = match codec {
                    CodecType::H264 => sys::android::CodecType::H264,
                    CodecType::H265 => sys::android::CodecType::H265,
                    CodecType::Av1 => unreachable!(),
                };
                DecoderInner::Android(sys::android::AndroidDecoder::new(
                    android_codec,
                    config,
                    width,
                    height,
                )?)
            }

            #[cfg(target_os = "windows")]
            CodecType::H264 | CodecType::H265 => {
                let windows_codec = match codec {
                    CodecType::H264 => sys::windows::CodecType::H264,
                    CodecType::H265 => sys::windows::CodecType::H265,
                    CodecType::Av1 => unreachable!(),
                };
                DecoderInner::Windows(sys::windows::WindowsDecoder::new(
                    windows_codec,
                    config,
                    width,
                    height,
                )?)
            }

            #[cfg(target_os = "linux")]
            CodecType::H264 | CodecType::H265 => {
                let linux_codec = match codec {
                    CodecType::H264 => sys::linux::CodecType::H264,
                    CodecType::H265 => sys::linux::CodecType::H265,
                    CodecType::Av1 => unreachable!(),
                };
                DecoderInner::Linux(sys::linux::LinuxDecoder::new(
                    linux_codec,
                    config,
                    width,
                    height,
                )?)
            }

            #[cfg(not(any(
                target_vendor = "apple",
                target_os = "android",
                target_os = "windows",
                target_os = "linux"
            )))]
            CodecType::H264 | CodecType::H265 => {
                return Err(CodecError::Unsupported(format!(
                    "{codec:?} hardware decoding not available on this platform"
                )));
            }

            #[cfg(all(
                feature = "software-fallback",
                not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
            ))]
            CodecType::Av1 => DecoderInner::Av1(software::av1::Av1Decoder::new()?),

            #[cfg(not(all(
                feature = "software-fallback",
                not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
            )))]
            CodecType::Av1 => {
                return Err(CodecError::Unsupported(
                    "AV1 software decoding not available on this platform".into(),
                ));
            }
        };

        Ok(Self { inner })
    }

    /// Decode compressed video data.
    ///
    /// Returns a streaming iterator yielding decoded frames as opaque [`DecodedFrame`] types.
    /// Use [`DecodedFrame::to_gpu_frame`] to convert to GPU textures on your device.
    pub fn decode(&mut self, data: &[u8]) -> DecodeStream {
        let result = self.decode_inner(data);
        match result {
            Ok(frames) => DecodeStream {
                inner: DecodeStreamInner::Frames(frames.into_iter()),
            },
            Err(e) => DecodeStream {
                inner: DecodeStreamInner::Error(Some(e)),
            },
        }
    }

    fn decode_inner(&mut self, data: &[u8]) -> Result<Vec<DecodedFrame>, CodecError> {
        match &mut self.inner {
            #[cfg(target_vendor = "apple")]
            DecoderInner::Apple(dec) => {
                let surfaces = dec.decode_to_iosurface(data)?;
                let mut frames = Vec::with_capacity(surfaces.len());
                for surface in surfaces {
                    let frame = DecodedFrame::from_iosurface(
                        surface.surface,
                        surface.width,
                        surface.height,
                        surface.timestamp_ns,
                    );
                    frames.push(frame);
                }
                Ok(frames)
            }

            #[cfg(target_os = "android")]
            DecoderInner::Android(dec) => {
                let android_frames = dec.decode(data)?;
                let mut frames = Vec::with_capacity(android_frames.len());
                for android_frame in android_frames {
                    let frame = DecodedFrame::from_nv12_data(
                        android_frame.data,
                        android_frame.width,
                        android_frame.height,
                        android_frame.timestamp_ns,
                    );
                    frames.push(frame);
                }
                Ok(frames)
            }

            #[cfg(target_os = "windows")]
            DecoderInner::Windows(dec) => {
                let windows_frames = dec.decode(data)?;
                let mut frames = Vec::with_capacity(windows_frames.len());
                for windows_frame in windows_frames {
                    let frame = DecodedFrame::from_nv12_data(
                        windows_frame.data,
                        windows_frame.width,
                        windows_frame.height,
                        windows_frame.timestamp_ns,
                    );
                    frames.push(frame);
                }
                Ok(frames)
            }

            #[cfg(target_os = "linux")]
            DecoderInner::Linux(dec) => {
                let linux_frames = dec.decode(data)?;
                let mut frames = Vec::with_capacity(linux_frames.len());
                for linux_frame in linux_frames {
                    let frame = DecodedFrame::from_nv12_data(
                        linux_frame.data,
                        linux_frame.width,
                        linux_frame.height,
                        linux_frame.timestamp_ns,
                    );
                    frames.push(frame);
                }
                Ok(frames)
            }

            #[cfg(all(
                feature = "software-fallback",
                not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
            ))]
            DecoderInner::Av1(dec) => {
                let cpu_frames = dec.decode(data)?;
                let mut frames = Vec::with_capacity(cpu_frames.len());
                for cpu_frame in cpu_frames {
                    let frame = DecodedFrame::from_nv12_data(
                        cpu_frame.data,
                        cpu_frame.width,
                        cpu_frame.height,
                        cpu_frame.timestamp_ns,
                    );
                    frames.push(frame);
                }
                Ok(frames)
            }
        }
    }

    /// Decode compressed video data directly into a provided buffer.
    ///
    /// This is the zero-copy path for unified memory systems (Apple Silicon, integrated GPUs).
    /// The buffer should be a mapped wgpu buffer slice. Returns a streaming iterator of frame info.
    pub fn decode_into(&mut self, data: &[u8], output: &mut [u8]) -> DecodeIntoStream {
        let result = self.decode_into_inner(data, output);
        match result {
            Ok(infos) => DecodeIntoStream {
                inner: DecodeIntoStreamInner::Infos(infos.into_iter()),
            },
            Err(e) => DecodeIntoStream {
                inner: DecodeIntoStreamInner::Error(Some(e)),
            },
        }
    }

    fn decode_into_inner(
        &mut self,
        data: &[u8],
        output: &mut [u8],
    ) -> Result<Vec<FrameInfo>, CodecError> {
        match &mut self.inner {
            #[cfg(target_vendor = "apple")]
            DecoderInner::Apple(dec) => {
                let surfaces = dec.decode_to_iosurface(data)?;
                let mut infos = Vec::with_capacity(surfaces.len());
                let mut offset = 0;

                for surface in &surfaces {
                    let width = surface.width;
                    let height = surface.height;
                    let y_size = (width * height) as usize;
                    let uv_size = y_size / 2;
                    let total_bytes = y_size + uv_size;

                    if offset + total_bytes > output.len() {
                        return Err(CodecError::DecodingFailed(format!(
                            "buffer too small: need {total_bytes} more bytes at offset {offset}"
                        )));
                    }

                    // Copy IOSurface to buffer using DecodedFrame helper
                    let frame = DecodedFrame::from_iosurface(
                        surface.surface.clone(),
                        width,
                        height,
                        surface.timestamp_ns,
                    );
                    frame.copy_to_buffer(&mut output[offset..offset + total_bytes]);

                    infos.push(FrameInfo {
                        width,
                        height,
                        timestamp: std::time::Duration::from_nanos(surface.timestamp_ns),
                        y_offset: offset,
                        uv_offset: offset + y_size,
                        total_bytes,
                    });

                    offset += total_bytes;
                }

                Ok(infos)
            }

            #[cfg(target_os = "android")]
            DecoderInner::Android(dec) => {
                let android_frames = dec.decode(data)?;
                copy_frames_to_buffer(
                    android_frames
                        .into_iter()
                        .map(|f| (f.data, f.width, f.height, f.timestamp_ns)),
                    output,
                )
            }

            #[cfg(target_os = "windows")]
            DecoderInner::Windows(dec) => {
                let windows_frames = dec.decode(data)?;
                copy_frames_to_buffer(
                    windows_frames
                        .into_iter()
                        .map(|f| (f.data, f.width, f.height, f.timestamp_ns)),
                    output,
                )
            }

            #[cfg(target_os = "linux")]
            DecoderInner::Linux(dec) => {
                let linux_frames = dec.decode(data)?;
                copy_frames_to_buffer(
                    linux_frames
                        .into_iter()
                        .map(|f| (f.data, f.width, f.height, f.timestamp_ns)),
                    output,
                )
            }

            #[cfg(all(
                feature = "software-fallback",
                not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
            ))]
            DecoderInner::Av1(dec) => {
                let cpu_frames = dec.decode(data)?;
                copy_frames_to_buffer(
                    cpu_frames
                        .into_iter()
                        .map(|f| (f.data, f.width, f.height, f.timestamp_ns)),
                    output,
                )
            }
        }
    }
}

/// Helper to copy decoded frames to an output buffer.
#[cfg(any(
    not(target_vendor = "apple"),
    all(
        target_vendor = "apple",
        not(any(target_os = "ios", target_os = "tvos", target_os = "watchos"))
    )
))]
fn copy_frames_to_buffer(
    frames: impl Iterator<Item = (Vec<u8>, u32, u32, u64)>,
    output: &mut [u8],
) -> Result<Vec<FrameInfo>, CodecError> {
    let mut infos = Vec::new();
    let mut offset = 0;

    for (data, width, height, timestamp_ns) in frames {
        let y_size = (width * height) as usize;
        let total_bytes = data.len();

        if offset + total_bytes > output.len() {
            return Err(CodecError::DecodingFailed(format!(
                "buffer too small: need {total_bytes} more bytes at offset {offset}"
            )));
        }

        output[offset..offset + total_bytes].copy_from_slice(&data);

        infos.push(FrameInfo {
            width,
            height,
            timestamp: std::time::Duration::from_nanos(timestamp_ns),
            y_offset: offset,
            uv_offset: offset + y_size,
            total_bytes,
        });

        offset += total_bytes;
    }

    Ok(infos)
}

/// Unified video encoder with automatic hardware/software selection.
pub struct Encoder {
    inner: EncoderInner,
}

enum EncoderInner {
    #[cfg(target_vendor = "apple")]
    Apple(sys::apple::AppleEncoder),
    #[cfg(target_os = "android")]
    Android(sys::android::AndroidEncoder),
    #[cfg(target_os = "windows")]
    Windows(sys::windows::WindowsEncoder),
    #[cfg(target_os = "linux")]
    Linux(sys::linux::LinuxEncoder),
    #[cfg(all(
        feature = "software-fallback",
        not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
    ))]
    Av1(Box<software::av1::Av1Encoder>),
}

impl std::fmt::Debug for Encoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Encoder").finish_non_exhaustive()
    }
}

impl Encoder {
    /// Create a new encoder.
    ///
    /// # Errors
    ///
    /// Returns error if no suitable encoder is available.
    pub fn new(codec: CodecType, width: u32, height: u32) -> Result<Self, CodecError> {
        let inner = match codec {
            #[cfg(target_vendor = "apple")]
            CodecType::H264 | CodecType::H265 => {
                let apple_codec = match codec {
                    CodecType::H264 => sys::apple::CodecType::H264,
                    CodecType::H265 => sys::apple::CodecType::H265,
                    CodecType::Av1 => unreachable!(),
                };
                EncoderInner::Apple(sys::apple::AppleEncoder::with_size(
                    apple_codec,
                    width,
                    height,
                )?)
            }

            #[cfg(target_os = "android")]
            CodecType::H264 | CodecType::H265 => {
                let android_codec = match codec {
                    CodecType::H264 => sys::android::CodecType::H264,
                    CodecType::H265 => sys::android::CodecType::H265,
                    CodecType::Av1 => unreachable!(),
                };
                EncoderInner::Android(sys::android::AndroidEncoder::new(
                    android_codec,
                    width,
                    height,
                )?)
            }

            #[cfg(target_os = "windows")]
            CodecType::H264 | CodecType::H265 => {
                let windows_codec = match codec {
                    CodecType::H264 => sys::windows::CodecType::H264,
                    CodecType::H265 => sys::windows::CodecType::H265,
                    CodecType::Av1 => unreachable!(),
                };
                EncoderInner::Windows(sys::windows::WindowsEncoder::new(
                    windows_codec,
                    width,
                    height,
                )?)
            }

            #[cfg(target_os = "linux")]
            CodecType::H264 | CodecType::H265 => {
                let linux_codec = match codec {
                    CodecType::H264 => sys::linux::CodecType::H264,
                    CodecType::H265 => sys::linux::CodecType::H265,
                    CodecType::Av1 => unreachable!(),
                };
                EncoderInner::Linux(sys::linux::LinuxEncoder::new(linux_codec, width, height)?)
            }

            #[cfg(not(any(
                target_vendor = "apple",
                target_os = "android",
                target_os = "windows",
                target_os = "linux"
            )))]
            CodecType::H264 | CodecType::H265 => {
                return Err(CodecError::Unsupported(format!(
                    "{codec:?} hardware encoding not available on this platform"
                )));
            }

            #[cfg(all(
                feature = "software-fallback",
                not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
            ))]
            CodecType::Av1 => EncoderInner::Av1(Box::new(software::av1::Av1Encoder::new(
                width as usize,
                height as usize,
            )?)),

            #[cfg(not(all(
                feature = "software-fallback",
                not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
            )))]
            CodecType::Av1 => {
                return Err(CodecError::Unsupported(
                    "AV1 software encoding not available on this platform".into(),
                ));
            }
        };

        Ok(Self { inner })
    }

    /// Encode a frame from NV12 data.
    ///
    /// Returns a streaming iterator yielding encoded packets.
    pub fn encode_nv12(&mut self, data: &[u8]) -> EncodeStream {
        let result = self.encode_nv12_inner(data);
        match result {
            Ok(packet) => EncodeStream {
                inner: EncodeStreamInner::Packets(vec![packet].into_iter()),
            },
            Err(e) => EncodeStream {
                inner: EncodeStreamInner::Error(Some(e)),
            },
        }
    }

    fn encode_nv12_inner(&mut self, data: &[u8]) -> Result<Vec<u8>, CodecError> {
        match &mut self.inner {
            #[cfg(target_vendor = "apple")]
            EncoderInner::Apple(enc) => enc.encode_nv12(data),

            #[cfg(target_os = "android")]
            EncoderInner::Android(enc) => enc.encode_nv12(data),

            #[cfg(target_os = "windows")]
            EncoderInner::Windows(enc) => enc.encode_nv12(data),

            #[cfg(target_os = "linux")]
            EncoderInner::Linux(enc) => enc.encode_nv12(data),

            #[cfg(all(
                feature = "software-fallback",
                not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
            ))]
            EncoderInner::Av1(enc) => enc.encode_nv12(data),
        }
    }

    /// Encode directly from an `IOSurface` (zero-copy, Apple only).
    ///
    /// Returns a streaming iterator yielding encoded packets.
    #[cfg(target_vendor = "apple")]
    pub fn encode_iosurface(&mut self, iosurface_ptr: u64) -> EncodeStream {
        let result = self.encode_iosurface_inner(iosurface_ptr);
        match result {
            Ok(packet) => EncodeStream {
                inner: EncodeStreamInner::Packets(vec![packet].into_iter()),
            },
            Err(e) => EncodeStream {
                inner: EncodeStreamInner::Error(Some(e)),
            },
        }
    }

    #[cfg(target_vendor = "apple")]
    fn encode_iosurface_inner(&mut self, iosurface_ptr: u64) -> Result<Vec<u8>, CodecError> {
        match &mut self.inner {
            EncoderInner::Apple(enc) => enc.encode_iosurface(iosurface_ptr),
            #[cfg(all(
                feature = "software-fallback",
                not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
            ))]
            EncoderInner::Av1(_) => Err(CodecError::Unsupported(
                "IOSurface encoding not supported for AV1".into(),
            )),
        }
    }

    /// Get codec configuration data (avcC/hvcC atom) if available.
    #[must_use]
    pub fn codec_config(&self) -> Option<Vec<u8>> {
        match &self.inner {
            #[cfg(target_vendor = "apple")]
            EncoderInner::Apple(enc) => enc.get_codec_config(),

            #[cfg(target_os = "android")]
            EncoderInner::Android(enc) => enc.get_codec_config(),

            #[cfg(target_os = "windows")]
            EncoderInner::Windows(enc) => enc.get_codec_config(),

            #[cfg(target_os = "linux")]
            EncoderInner::Linux(enc) => enc.get_codec_config(),

            #[cfg(all(
                feature = "software-fallback",
                not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
            ))]
            EncoderInner::Av1(_) => None, // AV1 doesn't use codec config atoms
        }
    }
}
