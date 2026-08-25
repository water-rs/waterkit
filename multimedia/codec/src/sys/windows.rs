//! Windows Media Foundation hardware encoding and decoding.

use crate::{
    CodecError, DecodePacket, DecodedPixelLayout, bitstream::NalStreamConverter,
    config::decoded_pixel_layout,
};
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::fmt;
use std::ptr;
use std::time::Duration;
use windows::Win32::Media::MediaFoundation::{
    IMF2DBuffer, IMFMediaType, IMFSample, IMFTransform, MF_E_NO_MORE_TYPES,
    MF_E_TRANSFORM_NEED_MORE_INPUT, MF_E_TRANSFORM_STREAM_CHANGE, MF_MT_FRAME_RATE,
    MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_MPEG_SEQUENCE_HEADER,
    MF_MT_SUBTYPE, MF_VERSION, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
    MFSTARTUP_NOSOCKET, MFShutdown, MFStartup, MFT_CATEGORY_VIDEO_DECODER,
    MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SORTANDFILTER,
    MFT_ENUM_FLAG_SYNCMFT, MFT_MESSAGE_COMMAND_DRAIN, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
    MFT_MESSAGE_NOTIFY_END_OF_STREAM, MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_OUTPUT_DATA_BUFFER,
    MFT_OUTPUT_STREAM_INFO, MFTEnumEx, MFVideoFormat_P010, MFVideoInterlace_Progressive,
};
use windows::Win32::System::Com::{
    COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::core::{GUID, Interface};

// Media Foundation GUIDs
const MF_MT_AVG_BITRATE: GUID = GUID::from_u128(0x20332624_fb0d_4d9e_bd0d_cbf6786c102e);

#[allow(non_upper_case_globals)]
const MFMediaType_Video: GUID = GUID::from_u128(0x73646976_0000_0010_8000_00aa00389b71);

#[allow(non_upper_case_globals)]
const MFVideoFormat_H264: GUID = GUID::from_u128(0x34363248_0000_0010_8000_00aa00389b71);

#[allow(non_upper_case_globals)]
const MFVideoFormat_HEVC: GUID = GUID::from_u128(0x43564548_0000_0010_8000_00aa00389b71);

#[allow(non_upper_case_globals)]
const MFVideoFormat_NV12: GUID = GUID::from_u128(0x3231564e_0000_0010_8000_00aa00389b71);

/// Internal codec type for Windows implementations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecType {
    H264,
    H265,
}

impl CodecType {
    const fn subtype(self) -> GUID {
        match self {
            Self::H264 => MFVideoFormat_H264,
            Self::H265 => MFVideoFormat_HEVC,
        }
    }
}

struct MediaFoundationSession;

impl MediaFoundationSession {
    fn new() -> Result<Self, CodecError> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(|error| {
                    CodecError::InitializationFailed(format!("CoInitializeEx failed: {error}"))
                })?;
            if let Err(error) = MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) {
                CoUninitialize();
                return Err(CodecError::InitializationFailed(format!(
                    "MFStartup failed: {error}"
                )));
            }
        }
        Ok(Self)
    }
}

impl Drop for MediaFoundationSession {
    fn drop(&mut self) {
        unsafe {
            if let Err(error) = MFShutdown() {
                tracing::error!(%error, "MFShutdown failed");
            }
            CoUninitialize();
        }
    }
}

/// Decoded frame from Windows Media Foundation (NV12 format).
#[derive(Clone)]
pub struct WindowsFrame {
    /// NV12 data: Y plane followed by interleaved UV plane.
    pub data: Vec<u8>,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Presentation timestamp in nanoseconds.
    pub timestamp_ns: u64,
    /// Native bi-planar pixel layout.
    pub layout: DecodedPixelLayout,
}

impl fmt::Debug for WindowsFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WindowsFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("timestamp_ns", &self.timestamp_ns)
            .finish_non_exhaustive()
    }
}

/// Windows Media Foundation hardware decoder.
#[allow(clippy::non_send_fields_in_send_ty)]
pub struct WindowsDecoder {
    transform: IMFTransform,
    codec_type: CodecType,
    width: u32,
    height: u32,
    output_stream_info: MFT_OUTPUT_STREAM_INFO,
    output_layout: DecodedPixelLayout,
    input_bitstream: NalStreamConverter,
    /// The exact presentation time submitted for each Media Foundation sample
    /// time, keyed by that sample time.
    ///
    /// Media Foundation counts in 100ns units, so a presentation time derived
    /// from a container timescale does not survive the round trip: 1024 ticks
    /// at 12288/s is 83.333333ms going in and 83.3333ms coming back. Callers
    /// match a decoded frame to the packet they submitted by exact timestamp,
    /// and 33ns of quantization is enough to miss.
    submitted_times: BTreeMap<i64, Duration>,
    _media_foundation: MediaFoundationSession,
}

impl fmt::Debug for WindowsDecoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WindowsDecoder")
            .field("codec_type", &self.codec_type)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl WindowsDecoder {
    /// Create a new Windows hardware decoder.
    pub fn new(
        codec_type: CodecType,
        config: Option<&[u8]>,
        width: u32,
        height: u32,
    ) -> Result<Self, CodecError> {
        let media_foundation = MediaFoundationSession::new()?;

        unsafe {
            let input_bitstream = NalStreamConverter::new(codec_type == CodecType::H265, config)?;
            let input_type = create_video_type(codec_type.subtype(), width, height)?;
            if !input_bitstream.parameter_sets().is_empty() {
                input_type
                    .SetBlob(
                        &MF_MT_MPEG_SEQUENCE_HEADER,
                        input_bitstream.parameter_sets(),
                    )
                    .map_err(|error| {
                        CodecError::InitializationFailed(format!(
                            "SetBlob codec configuration failed: {error}"
                        ))
                    })?;
            }
            let output_layout = decoded_pixel_layout(codec_type == CodecType::H265, config)?;
            let output_subtype = match output_layout {
                DecodedPixelLayout::Nv12 => MFVideoFormat_NV12,
                DecodedPixelLayout::P010 => MFVideoFormat_P010,
            };
            let output_type = create_video_type(output_subtype, width, height)?;

            let transform = activate_hardware_transform(
                MFT_CATEGORY_VIDEO_DECODER,
                create_register_type_info(codec_type.subtype()),
                create_register_type_info(output_subtype),
                "decoder",
            )?;

            transform.SetInputType(0, &input_type, 0).map_err(|e| {
                CodecError::InitializationFailed(format!("SetInputType failed: {e}"))
            })?;

            transform.SetOutputType(0, &output_type, 0).map_err(|e| {
                CodecError::InitializationFailed(format!("SetOutputType failed: {e}"))
            })?;

            let output_stream_info = transform.GetOutputStreamInfo(0).map_err(|e| {
                CodecError::InitializationFailed(format!("GetOutputStreamInfo: {e}"))
            })?;

            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .ok();
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .ok();

            Ok(Self {
                transform,
                codec_type,
                width,
                height,
                output_stream_info,
                output_layout,
                input_bitstream,
                submitted_times: BTreeMap::new(),
                _media_foundation: media_foundation,
            })
        }
    }

    /// Decode compressed video data.
    pub fn decode(&mut self, packet: DecodePacket<'_>) -> Result<Vec<WindowsFrame>, CodecError> {
        unsafe {
            // In-band, not just in the media type: the Media Foundation
            // transform accepts `MF_MT_MPEG_SEQUENCE_HEADER` and still waits for
            // an SPS in the elementary stream, consuming every sample, emitting
            // no frame and reporting no error.
            let annex_b = self
                .input_bitstream
                .convert_sample_with_parameter_sets(packet.data())?;
            let input_sample = create_sample(&annex_b)?;
            let time_100ns =
                i64::try_from(packet.presentation_time().as_nanos() / 100).map_err(|_| {
                    CodecError::DecodingFailed("presentation timestamp exceeds i64".into())
                })?;
            input_sample.SetSampleTime(time_100ns).map_err(|error| {
                CodecError::DecodingFailed(format!("SetSampleTime failed: {error}"))
            })?;
            self.submitted_times
                .insert(time_100ns, packet.presentation_time());

            self.transform
                .ProcessInput(0, &input_sample, 0)
                .map_err(|e| CodecError::DecodingFailed(format!("ProcessInput: {e}")))?;

            self.collect_output()
        }
    }

    /// Drains every delayed Media Foundation output frame.
    pub fn drain(&mut self) -> Result<Vec<WindowsFrame>, CodecError> {
        unsafe {
            self.transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0)
                .map_err(|error| {
                    CodecError::DecodingFailed(format!("notify end of stream failed: {error}"))
                })?;
            self.transform
                .ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0)
                .map_err(|error| {
                    CodecError::DecodingFailed(format!("start decoder drain failed: {error}"))
                })?;
            let frames = self.collect_output();
            // Anything still recorded here belongs to a sample the decoder
            // never emitted, and the stream is over; keeping it would grow the
            // map for the life of the decoder.
            self.submitted_times.clear();
            frames
        }
    }

    unsafe fn collect_output(&mut self) -> Result<Vec<WindowsFrame>, CodecError> {
        let mut frames = Vec::new();
        loop {
            let output_sample = if self.output_stream_info.dwFlags & 0x100 != 0 {
                None
            } else {
                let packed_len = self.output_layout.packed_len(self.width, self.height);
                Some(create_empty_sample(
                    (self.output_stream_info.cbSize as usize).max(packed_len),
                )?)
            };

            let mut output_buffer = MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: std::mem::ManuallyDrop::new(output_sample),
                dwStatus: 0,
                pEvents: std::mem::ManuallyDrop::new(None),
            };

            let mut status = 0u32;
            let result = unsafe {
                self.transform.ProcessOutput(
                    0,
                    std::slice::from_mut(&mut output_buffer),
                    &raw mut status,
                )
            };
            let output_sample = take_output_sample(&mut output_buffer);

            match result {
                Ok(()) => {
                    if let Some(sample) = output_sample.as_ref() {
                        let mut frame = extract_biplanar_frame(
                            sample,
                            self.width,
                            self.height,
                            self.output_layout,
                        )?;
                        // Hand back the presentation time that went in, not the
                        // one Media Foundation's 100ns grid rounded it to.
                        let sample_time =
                            i64::try_from(frame.timestamp_ns / 100).map_err(|_| {
                                CodecError::DecodingFailed("decoded sample time exceeds i64".into())
                            })?;
                        if let Some(submitted) = self.submitted_times.remove(&sample_time) {
                            frame.timestamp_ns =
                                u64::try_from(submitted.as_nanos()).map_err(|_| {
                                    CodecError::DecodingFailed(
                                        "submitted presentation time exceeds u64 nanoseconds"
                                            .into(),
                                    )
                                })?;
                        }
                        frames.push(frame);
                    }
                }
                Err(error) if error.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(frames),
                Err(error) if error.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    self.renegotiate_output_type()?;
                }
                Err(error) => {
                    return Err(CodecError::DecodingFailed(format!(
                        "ProcessOutput: {error}"
                    )));
                }
            }
        }
    }

    fn renegotiate_output_type(&mut self) -> Result<(), CodecError> {
        let expected_subtype = output_subtype(self.output_layout);
        let mut index = 0;
        loop {
            let media_type = match unsafe { self.transform.GetOutputAvailableType(0, index) } {
                Ok(media_type) => media_type,
                Err(error) if error.code() == MF_E_NO_MORE_TYPES => {
                    return Err(CodecError::Unsupported(format!(
                        "Media Foundation stream change has no {expected_subtype:?} output type"
                    )));
                }
                Err(error) => {
                    return Err(CodecError::DecodingFailed(format!(
                        "GetOutputAvailableType failed during stream change: {error}"
                    )));
                }
            };
            index += 1;
            let subtype = unsafe { media_type.GetGUID(&MF_MT_SUBTYPE) }.map_err(|error| {
                CodecError::DecodingFailed(format!(
                    "stream-change output type has no subtype: {error}"
                ))
            })?;
            if subtype != expected_subtype {
                continue;
            }
            let frame_size =
                unsafe { media_type.GetUINT64(&MF_MT_FRAME_SIZE) }.map_err(|error| {
                    CodecError::DecodingFailed(format!(
                        "stream-change output type has no frame size: {error}"
                    ))
                })?;
            let width = u32::try_from(frame_size >> 32).expect("frame width is stored as u32");
            let height = u32::try_from(frame_size & u64::from(u32::MAX))
                .expect("frame height is stored as u32");
            unsafe { self.transform.SetOutputType(0, &media_type, 0) }.map_err(|error| {
                CodecError::DecodingFailed(format!(
                    "SetOutputType failed during stream change: {error}"
                ))
            })?;
            self.width = width;
            self.height = height;
            self.output_stream_info =
                unsafe { self.transform.GetOutputStreamInfo(0) }.map_err(|error| {
                    CodecError::DecodingFailed(format!(
                        "GetOutputStreamInfo failed after stream change: {error}"
                    ))
                })?;
            return Ok(());
        }
    }
}

/// Windows Media Foundation hardware encoder.
#[allow(clippy::non_send_fields_in_send_ty)]
pub struct WindowsEncoder {
    transform: IMFTransform,
    codec_type: CodecType,
    width: u32,
    height: u32,
    frame_count: i64,
    output_stream_info: MFT_OUTPUT_STREAM_INFO,
    codec_config: Option<Vec<u8>>,
    _media_foundation: MediaFoundationSession,
}

impl fmt::Debug for WindowsEncoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WindowsEncoder")
            .field("codec_type", &self.codec_type)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("frame_count", &self.frame_count)
            .finish_non_exhaustive()
    }
}

impl WindowsEncoder {
    /// Create a new Windows hardware encoder.
    pub fn new(codec_type: CodecType, width: u32, height: u32) -> Result<Self, CodecError> {
        let media_foundation = MediaFoundationSession::new()?;

        unsafe {
            let transform = activate_hardware_transform(
                MFT_CATEGORY_VIDEO_ENCODER,
                create_register_type_info(MFVideoFormat_NV12),
                create_register_type_info(codec_type.subtype()),
                "encoder",
            )?;

            let output_type = create_video_type(codec_type.subtype(), width, height)?;
            output_type.SetUINT32(&MF_MT_AVG_BITRATE, 4_000_000).ok();

            transform.SetOutputType(0, &output_type, 0).map_err(|e| {
                CodecError::InitializationFailed(format!("SetOutputType failed: {e}"))
            })?;

            let input_type = create_video_type(MFVideoFormat_NV12, width, height)?;
            transform.SetInputType(0, &input_type, 0).map_err(|e| {
                CodecError::InitializationFailed(format!("SetInputType failed: {e}"))
            })?;

            let output_stream_info = transform.GetOutputStreamInfo(0).map_err(|e| {
                CodecError::InitializationFailed(format!("GetOutputStreamInfo: {e}"))
            })?;

            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .ok();
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .ok();

            Ok(Self {
                transform,
                codec_type,
                width,
                height,
                frame_count: 0,
                output_stream_info,
                codec_config: None,
                _media_foundation: media_foundation,
            })
        }
    }

    /// Encode NV12 data to compressed video.
    pub fn encode_nv12(&mut self, nv12: &[u8]) -> Result<Vec<u8>, CodecError> {
        let y_size = self.width as usize * self.height as usize;
        let uv_size = y_size / 2;
        let expected_size = y_size + uv_size;

        if nv12.len() != expected_size {
            return Err(CodecError::EncodingFailed(format!(
                "NV12 data size {} doesn't match expected {} for {}x{}",
                nv12.len(),
                expected_size,
                self.width,
                self.height
            )));
        }

        unsafe {
            let input_sample = create_sample(nv12)?;
            let time_100ns = self.frame_count * 333_333;
            self.frame_count += 1;
            input_sample.SetSampleTime(time_100ns).ok();
            input_sample.SetSampleDuration(333_333).ok();

            self.transform
                .ProcessInput(0, &input_sample, 0)
                .map_err(|e| CodecError::EncodingFailed(format!("ProcessInput: {e}")))?;

            let mut encoded_data = Vec::new();

            loop {
                let output_sample = if self.output_stream_info.dwFlags & 0x100 != 0 {
                    None
                } else {
                    Some(create_empty_sample(
                        self.output_stream_info.cbSize as usize,
                    )?)
                };

                let mut output_buffer = MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: std::mem::ManuallyDrop::new(output_sample),
                    dwStatus: 0,
                    pEvents: std::mem::ManuallyDrop::new(None),
                };

                let mut status = 0u32;
                let result = self.transform.ProcessOutput(
                    0,
                    std::slice::from_mut(&mut output_buffer),
                    &raw mut status,
                );
                let output_sample = take_output_sample(&mut output_buffer);

                match result {
                    Ok(()) => {
                        if let Some(sample) = output_sample.as_ref()
                            && let Ok(data) = extract_sample_data(sample)
                        {
                            encoded_data.extend_from_slice(&data);
                        }
                    }
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => break,
                    Err(e) => {
                        return Err(CodecError::EncodingFailed(format!("ProcessOutput: {e}")));
                    }
                }
            }

            Ok(encoded_data)
        }
    }

    /// Get the codec configuration data if available.
    #[must_use]
    pub fn get_codec_config(&self) -> Option<Vec<u8>> {
        self.codec_config.clone()
    }
}

// Helper functions

fn create_video_type(subtype: GUID, width: u32, height: u32) -> Result<IMFMediaType, CodecError> {
    unsafe {
        let media_type: IMFMediaType = MFCreateMediaType()
            .map_err(|e| CodecError::InitializationFailed(format!("MFCreateMediaType: {e}")))?;

        media_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|e| CodecError::InitializationFailed(format!("SetGUID major: {e}")))?;

        media_type
            .SetGUID(&MF_MT_SUBTYPE, &raw const subtype)
            .map_err(|e| CodecError::InitializationFailed(format!("SetGUID subtype: {e}")))?;

        let frame_size = (u64::from(width) << 32) | u64::from(height);
        media_type
            .SetUINT64(&MF_MT_FRAME_SIZE, frame_size)
            .map_err(|e| CodecError::InitializationFailed(format!("SetUINT64 frame_size: {e}")))?;

        let frame_rate = (30u64 << 32) | 1u64;
        media_type
            .SetUINT64(&MF_MT_FRAME_RATE, frame_rate)
            .map_err(|e| CodecError::InitializationFailed(format!("SetUINT64 frame_rate: {e}")))?;

        media_type
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(|e| CodecError::InitializationFailed(format!("SetUINT32 interlace: {e}")))?;

        Ok(media_type)
    }
}

const fn create_register_type_info(
    subtype: GUID,
) -> windows::Win32::Media::MediaFoundation::MFT_REGISTER_TYPE_INFO {
    windows::Win32::Media::MediaFoundation::MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: subtype,
    }
}

const fn output_subtype(layout: DecodedPixelLayout) -> GUID {
    match layout {
        DecodedPixelLayout::Nv12 => MFVideoFormat_NV12,
        DecodedPixelLayout::P010 => MFVideoFormat_P010,
    }
}

fn activate_hardware_transform(
    category: GUID,
    input_type: windows::Win32::Media::MediaFoundation::MFT_REGISTER_TYPE_INFO,
    output_type: windows::Win32::Media::MediaFoundation::MFT_REGISTER_TYPE_INFO,
    kind: &str,
) -> Result<IMFTransform, CodecError> {
    unsafe {
        let mut count = 0u32;
        let mut activates = ptr::null_mut();
        MFTEnumEx(
            category,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER | MFT_ENUM_FLAG_SYNCMFT,
            Some(&raw const input_type),
            Some(&raw const output_type),
            &raw mut activates,
            &raw mut count,
        )
        .map_err(|error| {
            CodecError::InitializationFailed(format!("MFTEnumEx {kind} discovery failed: {error}"))
        })?;

        if count == 0 {
            if !activates.is_null() {
                CoTaskMemFree(Some(activates.cast::<c_void>()));
            }
            return Err(CodecError::Unsupported(format!(
                "no hardware {kind} is available for the requested formats"
            )));
        }
        if activates.is_null() {
            return Err(CodecError::InitializationFailed(format!(
                "MFTEnumEx returned {count} {kind} activations through a null array"
            )));
        }

        let slots = std::slice::from_raw_parts_mut(activates, count as usize);
        let activation_objects = slots
            .iter_mut()
            .filter_map(Option::take)
            .collect::<Vec<_>>();
        CoTaskMemFree(Some(activates.cast::<c_void>()));

        let activate = activation_objects.into_iter().next().ok_or_else(|| {
            CodecError::InitializationFailed(format!(
                "MFTEnumEx returned no usable {kind} activation"
            ))
        })?;
        activate.ActivateObject().map_err(|error| {
            CodecError::InitializationFailed(format!("hardware {kind} activation failed: {error}"))
        })
    }
}

fn take_output_sample(output: &mut MFT_OUTPUT_DATA_BUFFER) -> Option<IMFSample> {
    unsafe {
        let sample = std::mem::ManuallyDrop::take(&mut output.pSample);
        drop(std::mem::ManuallyDrop::take(&mut output.pEvents));
        sample
    }
}

fn create_sample(data: &[u8]) -> Result<IMFSample, CodecError> {
    unsafe {
        let sample: IMFSample = MFCreateSample()
            .map_err(|e| CodecError::InitializationFailed(format!("MFCreateSample: {e}")))?;

        let len = u32::try_from(data.len())
            .map_err(|_| CodecError::EncodingFailed("data too large for u32".into()))?;

        let buffer = MFCreateMemoryBuffer(len)
            .map_err(|e| CodecError::InitializationFailed(format!("MFCreateMemoryBuffer: {e}")))?;

        let mut buffer_ptr = ptr::null_mut();
        buffer
            .Lock(&raw mut buffer_ptr, None, None)
            .map_err(|e| CodecError::InitializationFailed(format!("Buffer Lock: {e}")))?;

        ptr::copy_nonoverlapping(data.as_ptr(), buffer_ptr, data.len());

        buffer
            .SetCurrentLength(len)
            .map_err(|e| CodecError::InitializationFailed(format!("SetCurrentLength: {e}")))?;

        buffer
            .Unlock()
            .map_err(|e| CodecError::InitializationFailed(format!("Buffer Unlock: {e}")))?;

        sample
            .AddBuffer(&buffer)
            .map_err(|e| CodecError::InitializationFailed(format!("AddBuffer: {e}")))?;

        Ok(sample)
    }
}

fn create_empty_sample(size: usize) -> Result<IMFSample, CodecError> {
    unsafe {
        let sample: IMFSample = MFCreateSample()
            .map_err(|e| CodecError::InitializationFailed(format!("MFCreateSample: {e}")))?;

        let len = u32::try_from(size)
            .map_err(|_| CodecError::EncodingFailed("size too large for u32".into()))?;

        let buffer = MFCreateMemoryBuffer(len)
            .map_err(|e| CodecError::InitializationFailed(format!("MFCreateMemoryBuffer: {e}")))?;

        sample
            .AddBuffer(&buffer)
            .map_err(|e| CodecError::InitializationFailed(format!("AddBuffer: {e}")))?;

        Ok(sample)
    }
}

#[allow(clippy::cast_sign_loss)]
fn extract_biplanar_frame(
    sample: &IMFSample,
    width: u32,
    height: u32,
    layout: DecodedPixelLayout,
) -> Result<WindowsFrame, CodecError> {
    unsafe {
        let buffer = sample
            .GetBufferByIndex(0)
            .map_err(|e| CodecError::DecodingFailed(format!("GetBufferByIndex: {e}")))?;

        let expected_size = layout.packed_len(width, height);
        let data = if let Ok(buffer_2d) = buffer.cast::<IMF2DBuffer>() {
            copy_2d_biplanar_buffer(&buffer_2d, width, height, layout)?
        } else {
            let mut buffer_ptr = ptr::null_mut();
            let mut current_len = 0u32;
            buffer
                .Lock(&raw mut buffer_ptr, None, Some(&raw mut current_len))
                .map_err(|error| CodecError::DecodingFailed(format!("Lock: {error}")))?;
            let current_len = current_len as usize;
            if current_len != expected_size {
                buffer.Unlock().map_err(|error| {
                    CodecError::DecodingFailed(format!("Unlock after size mismatch: {error}"))
                })?;
                return Err(CodecError::DecodingFailed(format!(
                    "Media Foundation output has {current_len} bytes; expected {expected_size} for {width}x{height} {layout:?}"
                )));
            }
            let mut data = vec![0_u8; expected_size];
            ptr::copy_nonoverlapping(buffer_ptr, data.as_mut_ptr(), expected_size);
            buffer
                .Unlock()
                .map_err(|error| CodecError::DecodingFailed(format!("Unlock: {error}")))?;
            data
        };

        let sample_time = sample.GetSampleTime().map_err(|error| {
            CodecError::DecodingFailed(format!("decoded sample has no PTS: {error}"))
        })?;
        let timestamp_ns = u64::try_from(sample_time)
            .map_err(|_| {
                CodecError::DecodingFailed("decoded sample returned a negative PTS".into())
            })?
            .saturating_mul(100);

        Ok(WindowsFrame {
            data,
            width,
            height,
            timestamp_ns,
            layout,
        })
    }
}

fn copy_2d_biplanar_buffer(
    buffer: &IMF2DBuffer,
    width: u32,
    height: u32,
    layout: DecodedPixelLayout,
) -> Result<Vec<u8>, CodecError> {
    unsafe {
        let mut scanline = ptr::null_mut();
        let mut pitch = 0_i32;
        buffer
            .Lock2D(&raw mut scanline, &raw mut pitch)
            .map_err(|error| CodecError::DecodingFailed(format!("Lock2D failed: {error}")))?;

        let result = (|| {
            if scanline.is_null() {
                return Err(CodecError::DecodingFailed(
                    "Lock2D returned a null scanline".into(),
                ));
            }
            let pitch = usize::try_from(pitch).map_err(|_| {
                CodecError::DecodingFailed(format!(
                    "Media Foundation returned negative YUV pitch {pitch}"
                ))
            })?;
            let row_bytes = layout.bytes_per_row(width);
            if pitch < row_bytes {
                return Err(CodecError::DecodingFailed(format!(
                    "Media Foundation YUV pitch {pitch} is smaller than row size {row_bytes}"
                )));
            }

            let y_rows = height as usize;
            let uv_rows = y_rows / 2;
            let y_size = row_bytes * y_rows;
            let mut data = vec![0_u8; layout.packed_len(width, height)];
            for row in 0..y_rows {
                ptr::copy_nonoverlapping(
                    scanline.add(row * pitch),
                    data.as_mut_ptr().add(row * row_bytes),
                    row_bytes,
                );
            }
            let uv_scanline = scanline.add(pitch * y_rows);
            for row in 0..uv_rows {
                ptr::copy_nonoverlapping(
                    uv_scanline.add(row * pitch),
                    data.as_mut_ptr().add(y_size + row * row_bytes),
                    row_bytes,
                );
            }
            Ok(data)
        })();

        buffer
            .Unlock2D()
            .map_err(|error| CodecError::DecodingFailed(format!("Unlock2D failed: {error}")))?;
        result
    }
}

fn extract_sample_data(sample: &IMFSample) -> Result<Vec<u8>, CodecError> {
    unsafe {
        let buffer = sample
            .GetBufferByIndex(0)
            .map_err(|e| CodecError::EncodingFailed(format!("GetBufferByIndex: {e}")))?;

        let mut buffer_ptr = ptr::null_mut();
        let mut current_len = 0u32;

        buffer
            .Lock(&raw mut buffer_ptr, None, Some(&raw mut current_len))
            .map_err(|e| CodecError::EncodingFailed(format!("Lock: {e}")))?;

        let mut data = vec![0u8; current_len as usize];
        ptr::copy_nonoverlapping(buffer_ptr, data.as_mut_ptr(), current_len as usize);

        buffer
            .Unlock()
            .map_err(|e| CodecError::EncodingFailed(format!("Unlock: {e}")))?;

        Ok(data)
    }
}
