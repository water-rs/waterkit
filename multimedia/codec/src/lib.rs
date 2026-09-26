//! Hardware-aware video codec with deterministic timing and GPU texture output.
//!
//! This crate provides hardware-accelerated video encoding and decoding with
//! lazy GPU texture creation. Decoded frames are returned as an iterator of
//! opaque [`DecodedFrame`] types that can be converted to GPU textures when needed.
//!
//! # Features
//!
//! - `gpu` (default): uploads decoded frames to `wgpu` textures and converts
//!   them to linear RGBA on the GPU. Without it the crate decodes to CPU memory
//!   only, [`DecodedFrame::copy_to_buffer`] is the way out, and no `wgpu` is
//!   linked at all.
//! - `software-fallback` (default): AV1 encode and decode in software on
//!   desktop platforms.
//!
//! # Example
//!
//! ```ignore
//! // Create decoder without wgpu device
//! let mut decoder = Decoder::new(CodecType::H265, config, 1920, 1080)?;
//!
//! // Decode returns a streaming iterator (no GPU allocation yet)
//! let packet = DecodePacket::new(compressed_data, presentation_time);
//! for frame in decoder.decode(packet) {
//!     let frame = frame?;
//!     let gpu_frame = frame.to_gpu_frame(&my_device, &my_queue);
//!
//!     // Use YUV textures directly in shader
//!     let y = gpu_frame.y_texture();
//!     let uv = gpu_frame.uv_texture();
//!
//!     // Or convert to RGBA on GPU
//!     let rgba = gpu_frame.to_linear_rgba(&my_device, &my_queue, color_info);
//! }
//! ```
//!
//! # Mapped Buffer Path
//!
//! Use [`Decoder::decode_into`] when a caller needs tightly packed decoded planes
//! in a mapped buffer. This path performs a copy from native decoder storage and
//! needs no GPU device, so it is available without the `gpu` feature.
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

#[cfg(any(
    test,
    waterkit_hw_codec_android,
    target_os = "linux",
    waterkit_hw_codec_windows
))]
mod bitstream;
mod color;
// The VA-API backend reads its output layout back from the negotiated
// `DecodedFormat`, so Linux never has to infer it from the codec configuration.
#[cfg(any(
    test,
    waterkit_hw_codec_apple,
    waterkit_hw_codec_android,
    waterkit_hw_codec_windows
))]
mod config;
mod frame;
mod image;
#[cfg(target_os = "android")]
pub(crate) mod image_android;
#[cfg(target_vendor = "apple")]
mod image_apple;
mod software;
mod sys;

#[cfg(any(
    test,
    waterkit_hw_codec_android,
    target_os = "linux",
    waterkit_hw_codec_windows
))]
pub use bitstream::{ConvertedProtectedSample, NalStreamConverter, annex_b_to_length_prefixed};
pub use color::{
    ColorOutputTarget, SDR_REFERENCE_WHITE_NITS, VideoColorUniform, YUV_COLOR_SHADER_WGSL,
    video_color_uniform,
};
pub use frame::{DecodedFrame, DecodedPixelLayout};
#[cfg(feature = "gpu")]
pub use frame::{DecodedFrameUploader, GpuFrame, LinearRgbaConverter};
pub use image::{DecodedImage, DecodedPixelFormat, decode_image, decode_image_platform};

use std::{time::Duration, vec::IntoIter};
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

/// One compressed access unit and its presentation timestamp.
///
/// Keeping timing attached to the bytes prevents platform decoders from
/// silently inventing timestamps, which breaks B-frame reordering and A/V sync.
#[derive(Debug, Clone, Copy)]
pub struct DecodePacket<'a> {
    data: &'a [u8],
    presentation_time: Duration,
}

impl<'a> DecodePacket<'a> {
    /// Creates a compressed access unit with deterministic media timing.
    #[must_use]
    pub const fn new(data: &'a [u8], presentation_time: Duration) -> Self {
        Self {
            data,
            presentation_time,
        }
    }

    /// Returns the compressed access-unit bytes.
    #[must_use]
    pub const fn data(self) -> &'a [u8] {
        self.data
    }

    /// Returns the intended presentation timestamp.
    #[must_use]
    pub const fn presentation_time(self) -> Duration {
        self.presentation_time
    }
}

/// Frame info returned when decoding into a mapped buffer.
///
/// Use this with [`Decoder::decode_into`] to locate copied planes in a caller buffer.
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
/// Created by [`Encoder::encode_nv12`] or [`Encoder::encode_iosurface`]. Yields encoded data packets one at a time.
pub struct EncodeStream {
    inner: EncodeStreamInner,
}

enum EncodeStreamInner {
    /// Successful encode - yields the packet once.
    Packet(Option<Vec<u8>>),
    /// Encode error - yields the error once, then empty.
    Error(Option<CodecError>),
}

impl Iterator for EncodeStream {
    type Item = Result<Vec<u8>, CodecError>;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            EncodeStreamInner::Packet(packet) => packet.take().map(Ok),
            EncodeStreamInner::Error(err) => err.take().map(Err),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match &self.inner {
            EncodeStreamInner::Packet(Some(_)) | EncodeStreamInner::Error(Some(_)) => (1, Some(1)),
            EncodeStreamInner::Packet(None) | EncodeStreamInner::Error(None) => (0, Some(0)),
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
/// No GPU device is required until a decoded frame is uploaded to one.
pub struct Decoder {
    inner: DecoderInner,
}

enum DecoderInner {
    #[cfg(target_arch = "wasm32")]
    Unsupported,
    #[cfg(waterkit_hw_codec_apple)]
    Apple(sys::apple::AppleDecoder),
    #[cfg(waterkit_hw_codec_android)]
    Android(sys::android::AndroidDecoder),
    #[cfg(waterkit_hw_codec_windows)]
    Windows(sys::windows::WindowsDecoder),
    #[cfg(waterkit_hw_codec_vaapi)]
    Linux(sys::linux::LinuxDecoder),
    #[cfg(waterkit_av1_software)]
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
    /// later when the `gpu` feature is enabled.
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
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (config, width, height);
            let _unsupported = DecoderInner::Unsupported;
            return Err(CodecError::Unsupported(format!(
                "{codec:?} decoding is not supported by waterkit-codec on WebAssembly"
            )));
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            // With no hardware decoder backend these stay unused: the AV1
            // software decoder reads dimensions from the bitstream itself.
            #[cfg(not(waterkit_hw_codec))]
            let _ = (config, width, height);

            match codec {
                #[cfg(waterkit_hw_codec_apple)]
                CodecType::H264 | CodecType::H265 => Ok(Self {
                    inner: sys::apple::open_decoder(codec, config, width, height)?,
                }),

                #[cfg(waterkit_hw_codec_android)]
                CodecType::H264 | CodecType::H265 => Ok(Self {
                    inner: sys::android::open_decoder(codec, config, width, height)?,
                }),

                #[cfg(waterkit_hw_codec_windows)]
                CodecType::H264 | CodecType::H265 => Ok(Self {
                    inner: sys::windows::open_decoder(codec, config, width, height)?,
                }),

                #[cfg(waterkit_hw_codec_vaapi)]
                CodecType::H264 | CodecType::H265 => Ok(Self {
                    inner: sys::linux::open_decoder(codec, config, width, height)?,
                }),

                #[cfg(not(waterkit_hw_codec))]
                CodecType::H264 | CodecType::H265 => Err(CodecError::Unsupported(format!(
                    "{codec:?} hardware decoding not available on this platform"
                ))),

                #[cfg(waterkit_av1_software)]
                CodecType::Av1 => Ok(Self {
                    inner: DecoderInner::Av1(software::av1::Av1Decoder::new()?),
                }),

                #[cfg(not(waterkit_av1_software))]
                CodecType::Av1 => Err(CodecError::Unsupported(
                    "AV1 software decoding not available on this platform".into(),
                )),
            }
        }
    }

    /// Decode compressed video data.
    ///
    /// Returns a streaming iterator yielding decoded frames as opaque [`DecodedFrame`] types.
    /// With the `gpu` feature, convert them to GPU textures on your device;
    /// otherwise read them out with [`DecodedFrame::copy_to_buffer`].
    pub fn decode(&mut self, packet: DecodePacket<'_>) -> DecodeStream {
        let result = self.decode_inner(packet);
        match result {
            Ok(frames) => DecodeStream {
                inner: DecodeStreamInner::Frames(frames.into_iter()),
            },
            Err(e) => DecodeStream {
                inner: DecodeStreamInner::Error(Some(e)),
            },
        }
    }

    fn decode_inner(&mut self, packet: DecodePacket<'_>) -> Result<Vec<DecodedFrame>, CodecError> {
        #[cfg(any(target_arch = "wasm32", not(waterkit_any_codec)))]
        let _ = packet;
        match self.inner {
            #[cfg(target_arch = "wasm32")]
            DecoderInner::Unsupported => Err(CodecError::Unsupported(String::from(
                "video decoding is not supported by waterkit-codec on WebAssembly",
            ))),
            #[cfg(waterkit_hw_codec_apple)]
            DecoderInner::Apple(ref mut dec) => Ok(dec
                .decode_to_iosurface(packet)?
                .iter()
                .map(DecodedFrame::from_iosurface)
                .collect()),

            #[cfg(waterkit_hw_codec_android)]
            DecoderInner::Android(ref mut dec) => {
                let android_frames = dec.decode(packet)?;
                let mut frames = Vec::with_capacity(android_frames.len());
                for android_frame in android_frames {
                    let frame = DecodedFrame::from_biplanar_data(
                        android_frame.data,
                        android_frame.width,
                        android_frame.height,
                        android_frame.timestamp_ns,
                        android_frame.layout,
                    );
                    frames.push(frame);
                }
                Ok(frames)
            }

            #[cfg(waterkit_hw_codec_windows)]
            DecoderInner::Windows(ref mut dec) => {
                let windows_frames = dec.decode(packet)?;
                let mut frames = Vec::with_capacity(windows_frames.len());
                for windows_frame in windows_frames {
                    let frame = DecodedFrame::from_biplanar_data(
                        windows_frame.data,
                        windows_frame.width,
                        windows_frame.height,
                        windows_frame.timestamp_ns,
                        windows_frame.layout,
                    );
                    frames.push(frame);
                }
                Ok(frames)
            }

            #[cfg(waterkit_hw_codec_vaapi)]
            DecoderInner::Linux(ref mut dec) => {
                let linux_frames = dec.decode(packet)?;
                let mut frames = Vec::with_capacity(linux_frames.len());
                for linux_frame in linux_frames {
                    let frame = DecodedFrame::from_biplanar_data(
                        linux_frame.data,
                        linux_frame.width,
                        linux_frame.height,
                        linux_frame.timestamp_ns,
                        linux_frame.layout,
                    );
                    frames.push(frame);
                }
                Ok(frames)
            }

            #[cfg(waterkit_av1_software)]
            DecoderInner::Av1(ref mut dec) => {
                let cpu_frames = dec.decode(packet)?;
                let mut frames = Vec::with_capacity(cpu_frames.len());
                for cpu_frame in cpu_frames {
                    let frame = DecodedFrame::from_biplanar_data(
                        cpu_frame.data,
                        cpu_frame.width,
                        cpu_frame.height,
                        cpu_frame.timestamp_ns,
                        cpu_frame.layout,
                    );
                    frames.push(frame);
                }
                Ok(frames)
            }
        }
    }

    /// Signals end of stream and yields every delayed decoder output frame.
    ///
    /// A decoder must be rebuilt before submitting more packets after this call.
    pub fn drain(&mut self) -> DecodeStream {
        match self.drain_inner() {
            Ok(frames) => DecodeStream {
                inner: DecodeStreamInner::Frames(frames.into_iter()),
            },
            Err(error) => DecodeStream {
                inner: DecodeStreamInner::Error(Some(error)),
            },
        }
    }

    fn drain_inner(&mut self) -> Result<Vec<DecodedFrame>, CodecError> {
        match self.inner {
            #[cfg(target_arch = "wasm32")]
            DecoderInner::Unsupported => Err(CodecError::Unsupported(String::from(
                "video decoding is not supported by waterkit-codec on WebAssembly",
            ))),
            #[cfg(waterkit_hw_codec_apple)]
            DecoderInner::Apple(ref mut decoder) => Ok(decoder
                .drain()?
                .iter()
                .map(DecodedFrame::from_iosurface)
                .collect()),
            #[cfg(waterkit_hw_codec_android)]
            DecoderInner::Android(ref mut decoder) => Ok(decoder
                .drain()?
                .into_iter()
                .map(|frame| {
                    DecodedFrame::from_biplanar_data(
                        frame.data,
                        frame.width,
                        frame.height,
                        frame.timestamp_ns,
                        frame.layout,
                    )
                })
                .collect()),
            #[cfg(waterkit_hw_codec_windows)]
            DecoderInner::Windows(ref mut decoder) => Ok(decoder
                .drain()?
                .into_iter()
                .map(|frame| {
                    DecodedFrame::from_biplanar_data(
                        frame.data,
                        frame.width,
                        frame.height,
                        frame.timestamp_ns,
                        frame.layout,
                    )
                })
                .collect()),
            #[cfg(waterkit_hw_codec_vaapi)]
            DecoderInner::Linux(ref mut decoder) => Ok(decoder
                .drain()?
                .into_iter()
                .map(|frame| {
                    DecodedFrame::from_biplanar_data(
                        frame.data,
                        frame.width,
                        frame.height,
                        frame.timestamp_ns,
                        frame.layout,
                    )
                })
                .collect()),
            #[cfg(waterkit_av1_software)]
            DecoderInner::Av1(ref mut decoder) => Ok(decoder
                .drain()?
                .into_iter()
                .map(|frame| {
                    DecodedFrame::from_biplanar_data(
                        frame.data,
                        frame.width,
                        frame.height,
                        frame.timestamp_ns,
                        frame.layout,
                    )
                })
                .collect()),
        }
    }

    /// Decode compressed video data directly into a provided buffer.
    ///
    /// The buffer may be a mapped wgpu buffer slice. Native decoder storage is
    /// copied into it, and the returned frame info locates each plane.
    pub fn decode_into(&mut self, packet: DecodePacket<'_>, output: &mut [u8]) -> DecodeIntoStream {
        let result = self.decode_into_inner(packet, output);
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
        packet: DecodePacket<'_>,
        output: &mut [u8],
    ) -> Result<Vec<FrameInfo>, CodecError> {
        #[cfg(any(target_arch = "wasm32", not(waterkit_any_codec)))]
        let _ = (packet, output);
        match self.inner {
            #[cfg(target_arch = "wasm32")]
            DecoderInner::Unsupported => Err(CodecError::Unsupported(String::from(
                "video decoding is not supported by waterkit-codec on WebAssembly",
            ))),
            #[cfg(waterkit_hw_codec_apple)]
            DecoderInner::Apple(ref mut dec) => {
                let surfaces = dec.decode_to_iosurface(packet)?;
                let mut infos = Vec::with_capacity(surfaces.len());
                let mut offset = 0;

                for surface in &surfaces {
                    let width = surface.width;
                    let height = surface.height;
                    let y_size = surface.layout.bytes_per_row(width) * height as usize;
                    let total_bytes = surface.layout.packed_len(width, height);

                    if offset + total_bytes > output.len() {
                        return Err(CodecError::DecodingFailed(format!(
                            "buffer too small: need {total_bytes} more bytes at offset {offset}"
                        )));
                    }

                    // Copy IOSurface to buffer using DecodedFrame helper
                    let frame = DecodedFrame::from_iosurface(surface);
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

            #[cfg(waterkit_hw_codec_android)]
            DecoderInner::Android(ref mut dec) => {
                let android_frames = dec.decode(packet)?;
                copy_frames_to_buffer(
                    android_frames
                        .into_iter()
                        .map(|f| (f.data, f.width, f.height, f.timestamp_ns, f.layout)),
                    output,
                )
            }

            #[cfg(waterkit_hw_codec_windows)]
            DecoderInner::Windows(ref mut dec) => {
                let windows_frames = dec.decode(packet)?;
                copy_frames_to_buffer(
                    windows_frames
                        .into_iter()
                        .map(|f| (f.data, f.width, f.height, f.timestamp_ns, f.layout)),
                    output,
                )
            }

            #[cfg(waterkit_hw_codec_vaapi)]
            DecoderInner::Linux(ref mut dec) => {
                let linux_frames = dec.decode(packet)?;
                copy_frames_to_buffer(
                    linux_frames
                        .into_iter()
                        .map(|f| (f.data, f.width, f.height, f.timestamp_ns, f.layout)),
                    output,
                )
            }

            #[cfg(waterkit_av1_software)]
            DecoderInner::Av1(ref mut dec) => {
                let cpu_frames = dec.decode(packet)?;
                copy_frames_to_buffer(
                    cpu_frames
                        .into_iter()
                        .map(|f| (f.data, f.width, f.height, f.timestamp_ns, f.layout)),
                    output,
                )
            }
        }
    }
}

/// Helper to copy decoded frames to an output buffer.
#[cfg(waterkit_software_frames)]
fn copy_frames_to_buffer(
    frames: impl Iterator<Item = (Vec<u8>, u32, u32, u64, DecodedPixelLayout)>,
    output: &mut [u8],
) -> Result<Vec<FrameInfo>, CodecError> {
    let mut infos = Vec::new();
    let mut offset = 0;

    for (data, width, height, timestamp_ns, layout) in frames {
        let y_size = layout.bytes_per_row(width) * height as usize;
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

/// How an [`Encoder`] trades compression efficiency for latency.
///
/// The profile is chosen at construction because software encoders bake
/// their threading and speed preset into the context; it cannot be
/// switched on the fly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EncoderProfile {
    /// Compression-first encoding for offline jobs: balanced speed preset
    /// and a modest thread count (rav1e preset 6 on four threads).
    #[default]
    Offline,
    /// Latency-first encoding for live capture: the fastest settings each
    /// backend offers (rav1e's maximum speed preset on every available
    /// core; hardware encoders are already realtime).
    Realtime,
}

/// Unified video encoder with automatic hardware/software selection.
pub struct Encoder {
    inner: EncoderInner,
}

enum EncoderInner {
    #[cfg(target_arch = "wasm32")]
    Unsupported,
    #[cfg(waterkit_hw_codec_apple)]
    Apple(sys::apple::AppleEncoder),
    #[cfg(waterkit_hw_codec_android)]
    Android(sys::android::AndroidEncoder),
    #[cfg(waterkit_hw_codec_windows)]
    Windows(sys::windows::WindowsEncoder),
    #[cfg(waterkit_hw_codec_vaapi)]
    Linux(sys::linux::LinuxEncoder),
    #[cfg(waterkit_av1_software)]
    Av1(Box<software::av1::Av1Encoder>),
}

impl std::fmt::Debug for Encoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Encoder").finish_non_exhaustive()
    }
}

impl Encoder {
    /// Create a new encoder tuned for `profile`.
    ///
    /// # Errors
    ///
    /// Returns error if no suitable encoder is available.
    pub fn new(
        codec: CodecType,
        width: u32,
        height: u32,
        profile: EncoderProfile,
    ) -> Result<Self, CodecError> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (width, height, profile);
            let _unsupported = EncoderInner::Unsupported;
            return Err(CodecError::Unsupported(format!(
                "{codec:?} encoding is not supported by waterkit-codec on WebAssembly"
            )));
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            #[cfg(not(waterkit_any_codec))]
            let _ = (width, height);
            #[cfg(not(waterkit_av1_software))]
            let _ = profile;
            match codec {
                #[cfg(waterkit_hw_codec_apple)]
                CodecType::H264 | CodecType::H265 => Ok(Self {
                    inner: sys::apple::open_encoder(codec, width, height)?,
                }),

                #[cfg(waterkit_hw_codec_android)]
                CodecType::H264 | CodecType::H265 => Ok(Self {
                    inner: sys::android::open_encoder(codec, width, height)?,
                }),

                #[cfg(waterkit_hw_codec_windows)]
                CodecType::H264 | CodecType::H265 => Ok(Self {
                    inner: sys::windows::open_encoder(codec, width, height)?,
                }),

                #[cfg(waterkit_hw_codec_vaapi)]
                CodecType::H264 | CodecType::H265 => Ok(Self {
                    inner: sys::linux::open_encoder(codec, width, height)?,
                }),

                #[cfg(not(waterkit_hw_codec))]
                CodecType::H264 | CodecType::H265 => Err(CodecError::Unsupported(format!(
                    "{codec:?} hardware encoding not available on this platform"
                ))),

                #[cfg(waterkit_av1_software)]
                CodecType::Av1 => Ok(Self {
                    inner: EncoderInner::Av1(Box::new(software::av1::Av1Encoder::new(
                        width as usize,
                        height as usize,
                        profile,
                    )?)),
                }),

                #[cfg(not(waterkit_av1_software))]
                CodecType::Av1 => Err(CodecError::Unsupported(
                    "AV1 software encoding not available on this platform".into(),
                )),
            }
        }
    }

    /// Encode a frame from NV12 data.
    ///
    /// Returns a streaming iterator yielding encoded packets.
    pub fn encode_nv12(&mut self, data: &[u8]) -> EncodeStream {
        let result = self.encode_nv12_inner(data);
        match result {
            Ok(packet) => EncodeStream {
                inner: EncodeStreamInner::Packet(Some(packet)),
            },
            Err(e) => EncodeStream {
                inner: EncodeStreamInner::Error(Some(e)),
            },
        }
    }

    fn encode_nv12_inner(&mut self, data: &[u8]) -> Result<Vec<u8>, CodecError> {
        #[cfg(any(target_arch = "wasm32", not(waterkit_any_codec)))]
        let _ = data;
        match self.inner {
            #[cfg(target_arch = "wasm32")]
            EncoderInner::Unsupported => Err(CodecError::Unsupported(String::from(
                "video encoding is not supported by waterkit-codec on WebAssembly",
            ))),
            #[cfg(waterkit_hw_codec_apple)]
            EncoderInner::Apple(ref mut enc) => enc.encode_nv12(data),

            #[cfg(waterkit_hw_codec_android)]
            EncoderInner::Android(ref mut enc) => enc.encode_nv12(data),

            #[cfg(waterkit_hw_codec_windows)]
            EncoderInner::Windows(ref mut enc) => enc.encode_nv12(data),

            #[cfg(waterkit_hw_codec_vaapi)]
            EncoderInner::Linux(ref mut enc) => enc.encode_nv12(data),

            #[cfg(waterkit_av1_software)]
            EncoderInner::Av1(ref mut enc) => enc.encode_nv12(data),
        }
    }

    /// Encode directly from an `IOSurface` (zero-copy, Apple only).
    ///
    /// Returns a streaming iterator yielding encoded packets.
    #[cfg(waterkit_hw_codec_apple)]
    pub fn encode_iosurface(&mut self, iosurface_ptr: u64) -> EncodeStream {
        let result = self.encode_iosurface_inner(iosurface_ptr);
        match result {
            Ok(packet) => EncodeStream {
                inner: EncodeStreamInner::Packet(Some(packet)),
            },
            Err(e) => EncodeStream {
                inner: EncodeStreamInner::Error(Some(e)),
            },
        }
    }

    /// Drain frames the encoder still holds after the last `encode_nv12`.
    ///
    /// Hardware backends in this crate emit bitstream synchronously per
    /// `encode_nv12` call, so flushing them yields no packets. The software
    /// AV1 encoder queues input internally and only emits the held frames
    /// here; each returned packet is one encoded frame.
    ///
    /// # Errors
    /// Returns [`CodecError::EncodingFailed`] when the drain fails.
    #[cfg_attr(
        not(any(waterkit_av1_software, target_arch = "wasm32")),
        expect(
            clippy::missing_const_for_fn,
            reason = "only the software AV1 and wasm32 arms are non-const; the hardware-only builds reduce to `Ok(Vec::new())`"
        )
    )]
    pub fn flush(&mut self) -> Result<Vec<Vec<u8>>, CodecError> {
        match self.inner {
            #[cfg(target_arch = "wasm32")]
            EncoderInner::Unsupported => Err(CodecError::Unsupported(String::from(
                "video encoding is not supported by waterkit-codec on WebAssembly",
            ))),
            #[cfg(waterkit_hw_codec_apple)]
            EncoderInner::Apple(_) => Ok(Vec::new()),
            #[cfg(waterkit_hw_codec_android)]
            EncoderInner::Android(_) => Ok(Vec::new()),
            #[cfg(waterkit_hw_codec_windows)]
            EncoderInner::Windows(_) => Ok(Vec::new()),
            #[cfg(waterkit_hw_codec_vaapi)]
            EncoderInner::Linux(_) => Ok(Vec::new()),
            #[cfg(waterkit_av1_software)]
            EncoderInner::Av1(ref mut enc) => enc.flush(),
        }
    }

    #[cfg(waterkit_hw_codec_apple)]
    fn encode_iosurface_inner(&mut self, iosurface_ptr: u64) -> Result<Vec<u8>, CodecError> {
        match self.inner {
            EncoderInner::Apple(ref mut enc) => enc.encode_iosurface(iosurface_ptr),
            #[cfg(waterkit_av1_software)]
            EncoderInner::Av1(_) => Err(CodecError::Unsupported(
                "IOSurface encoding not supported for AV1".into(),
            )),
        }
    }

    /// Get codec configuration data (avcC/hvcC atom) if available.
    #[must_use]
    #[allow(
        clippy::missing_const_for_fn,
        reason = "non-const on backends that call through to a platform encoder; only the reduced no-vaapi build is const-eligible"
    )]
    pub fn codec_config(&self) -> Option<Vec<u8>> {
        match self.inner {
            #[cfg(target_arch = "wasm32")]
            EncoderInner::Unsupported => None,
            #[cfg(waterkit_hw_codec_apple)]
            EncoderInner::Apple(ref enc) => enc.get_codec_config(),

            #[cfg(waterkit_hw_codec_android)]
            EncoderInner::Android(ref enc) => enc.get_codec_config(),

            #[cfg(waterkit_hw_codec_windows)]
            EncoderInner::Windows(ref enc) => enc.get_codec_config(),

            #[cfg(waterkit_hw_codec_vaapi)]
            EncoderInner::Linux(ref enc) => enc.get_codec_config(),

            #[cfg(waterkit_av1_software)]
            EncoderInner::Av1(ref enc) => Some(enc.codec_config()),
        }
    }
}
