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
mod software;
mod sys;

pub use frame::{DecodedFrame, GpuFrame, YuvConverter};
<<<<<<< HEAD
pub use image::{DecodedImage, DecodedPixelFormat, decode_image};
=======
pub use image::{DecodedImage, DecodedPixelFormat, decode_image, decode_image_rgba8};
>>>>>>> main

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
    /// AV1 - hardware first, optional software fallback via `software-fallback` feature
    Av1,
}

/// Capability support level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SupportLevel {
    /// Capability is supported.
    Supported,
    /// Capability is not supported.
    Unsupported,
}

/// HDR support report for the current runtime platform/device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HdrSupport {
    /// 10-bit HDR decode capability.
    pub decode_10bit: SupportLevel,
    /// 10-bit HDR encode capability.
    pub encode_10bit: SupportLevel,
}

/// VA-API encoder support report (Linux only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VaapiEncodeSupport {
    /// H.264 VA-API encoder availability.
    pub h264: bool,
    /// H.265 VA-API encoder availability.
    pub h265: bool,
    /// AV1 VA-API encoder availability.
    pub av1: bool,
}

/// Query 10-bit HDR codec capability for current platform/device.
#[must_use]
pub fn check_hdr_support() -> HdrSupport {
    #[cfg(target_vendor = "apple")]
    {
        return sys::apple::check_hdr_support();
    }

    #[cfg(target_os = "android")]
    {
        return sys::android::check_hdr_support();
    }

    #[cfg(target_os = "windows")]
    {
        return sys::windows::check_hdr_support();
    }

    #[cfg(target_os = "linux")]
    {
        return sys::linux::check_hdr_support();
    }

    #[allow(unreachable_code)]
    HdrSupport {
        decode_10bit: SupportLevel::Unsupported,
        encode_10bit: SupportLevel::Unsupported,
    }
}

/// Query VA-API encoder availability for current runtime.
///
/// Returns `None` on non-Linux platforms.
#[must_use]
pub fn check_vaapi_encode_support() -> Option<VaapiEncodeSupport> {
    #[cfg(target_os = "linux")]
    {
        return Some(sys::linux::check_vaapi_encode_support());
    }

    #[allow(unreachable_code)]
    None
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
    /// Presentation timestamp in nanoseconds.
    pub timestamp_ns: u64,
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
///
/// For AV1 decode, fallback is automatic and silent:
/// - If stream config indicates HDR (10/12-bit) but hardware HDR decode is unsupported,
///   decoder prefers software AV1.
/// - If software AV1 is unavailable, decoder falls back to hardware decode as SDR.
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
    #[cfg(feature = "software-fallback")]
    Av1Software(software::av1::Av1Decoder),
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

            CodecType::Av1 => Self::new_av1_decoder(config, width, height)?,
        };

        Ok(Self { inner })
    }

    fn new_av1_decoder(
        config: Option<&[u8]>,
        width: u32,
        height: u32,
    ) -> Result<DecoderInner, CodecError> {
        // Silent policy:
        // 1) If AV1 stream indicates HDR and hardware HDR decode is unsupported, prefer software.
        // 2) If software is unavailable, fall back to hardware decode as SDR.
        if Self::av1_config_requests_hdr(config)
            && check_hdr_support().decode_10bit == SupportLevel::Unsupported
        {
            if let Ok(software) = Self::new_av1_software_decoder(CodecError::Unsupported(
                "AV1 HDR decode not supported by hardware".into(),
            )) {
                return Ok(software);
            }
            return Self::new_av1_hardware_decoder(config, width, height);
        }

        match Self::new_av1_hardware_decoder(config, width, height) {
            Ok(inner) => Ok(inner),
            Err(hw_error) => Self::new_av1_software_decoder(hw_error),
        }
    }

    fn av1_config_requests_hdr(config: Option<&[u8]>) -> bool {
        let Some(config) = config else {
            return false;
        };

        let payload = if config.len() > 8 && &config[4..8] == b"av1C" {
            &config[8..]
        } else {
            config
        };

        if payload.len() < 3 {
            return false;
        }

        // AV1CodecConfigurationRecord byte[2]:
        // bit6 = high_bitdepth, bit5 = twelve_bit
        let high_bitdepth = (payload[2] & 0x40) != 0;
        let twelve_bit = (payload[2] & 0x20) != 0;
        high_bitdepth || twelve_bit
    }

    fn new_av1_hardware_decoder(
        config: Option<&[u8]>,
        width: u32,
        height: u32,
    ) -> Result<DecoderInner, CodecError> {
        #[cfg(target_vendor = "apple")]
        {
            return sys::apple::AppleDecoder::new(
                sys::apple::CodecType::Av1,
                config,
                width,
                height,
            )
            .map(DecoderInner::Apple);
        }

        #[cfg(target_os = "android")]
        {
            return sys::android::AndroidDecoder::new(
                sys::android::CodecType::Av1,
                config,
                width,
                height,
            )
            .map(DecoderInner::Android);
        }

        #[cfg(target_os = "windows")]
        {
            return sys::windows::WindowsDecoder::new(
                sys::windows::CodecType::Av1,
                config,
                width,
                height,
            )
            .map(DecoderInner::Windows);
        }

        #[cfg(target_os = "linux")]
        {
            return sys::linux::LinuxDecoder::new(
                sys::linux::CodecType::Av1,
                config,
                width,
                height,
            )
            .map(DecoderInner::Linux);
        }

        #[allow(unreachable_code)]
        Err(CodecError::Unsupported(
            "AV1 hardware decoding not available on this platform".into(),
        ))
    }

    #[cfg(feature = "software-fallback")]
    fn new_av1_software_decoder(_hw_error: CodecError) -> Result<DecoderInner, CodecError> {
        Ok(DecoderInner::Av1Software(software::av1::Av1Decoder::new()?))
    }

    #[cfg(not(feature = "software-fallback"))]
    fn new_av1_software_decoder(hw_error: CodecError) -> Result<DecoderInner, CodecError> {
        Err(hw_error)
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

            #[cfg(feature = "software-fallback")]
            DecoderInner::Av1Software(dec) => {
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
                        timestamp_ns: surface.timestamp_ns,
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

            #[cfg(feature = "software-fallback")]
            DecoderInner::Av1Software(dec) => {
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
#[cfg(any(not(target_vendor = "apple"), feature = "software-fallback"))]
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
            timestamp_ns,
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
    #[cfg(feature = "software-fallback")]
    Av1Software(Box<software::av1::Av1Encoder>),
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

            CodecType::Av1 => Self::new_av1_encoder(width, height)?,
        };

        Ok(Self { inner })
    }

    fn new_av1_encoder(width: u32, height: u32) -> Result<EncoderInner, CodecError> {
        match Self::new_av1_hardware_encoder(width, height) {
            Ok(inner) => Ok(inner),
            Err(hw_error) => Self::new_av1_software_encoder(width, height, hw_error),
        }
    }

    fn new_av1_hardware_encoder(width: u32, height: u32) -> Result<EncoderInner, CodecError> {
        #[cfg(target_vendor = "apple")]
        {
            return sys::apple::AppleEncoder::with_size(sys::apple::CodecType::Av1, width, height)
                .map(EncoderInner::Apple);
        }

        #[cfg(target_os = "android")]
        {
            return sys::android::AndroidEncoder::new(sys::android::CodecType::Av1, width, height)
                .map(EncoderInner::Android);
        }

        #[cfg(target_os = "windows")]
        {
            return sys::windows::WindowsEncoder::new(sys::windows::CodecType::Av1, width, height)
                .map(EncoderInner::Windows);
        }

        #[cfg(target_os = "linux")]
        {
            return sys::linux::LinuxEncoder::new(sys::linux::CodecType::Av1, width, height)
                .map(EncoderInner::Linux);
        }

        #[allow(unreachable_code)]
        Err(CodecError::Unsupported(
            "AV1 hardware encoding not available on this platform".into(),
        ))
    }

    #[cfg(feature = "software-fallback")]
    fn new_av1_software_encoder(
        width: u32,
        height: u32,
        _hw_error: CodecError,
    ) -> Result<EncoderInner, CodecError> {
        Ok(EncoderInner::Av1Software(Box::new(
            software::av1::Av1Encoder::new(width as usize, height as usize)?,
        )))
    }

    #[cfg(not(feature = "software-fallback"))]
    fn new_av1_software_encoder(
        _width: u32,
        _height: u32,
        hw_error: CodecError,
    ) -> Result<EncoderInner, CodecError> {
        Err(hw_error)
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

            #[cfg(feature = "software-fallback")]
            EncoderInner::Av1Software(enc) => enc.encode_nv12(data),
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
            #[cfg(feature = "software-fallback")]
            EncoderInner::Av1Software(_) => Err(CodecError::Unsupported(
                "IOSurface encoding not supported for AV1".into(),
            )),
        }
    }

    /// Get codec configuration data (avcC/hvcC/av1C atom) if available.
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

            #[cfg(feature = "software-fallback")]
            EncoderInner::Av1Software(_) => None, // AV1 doesn't use codec config atoms
        }
    }
}
