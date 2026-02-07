//! Windows Media Foundation hardware encoding and decoding.

use crate::CodecError;
use std::fmt;
use std::ptr;
use windows::Win32::Media::MediaFoundation::{
    IMFMediaType, IMFSample, IMFTransform, MF_E_TRANSFORM_NEED_MORE_INPUT, MF_MT_FRAME_RATE,
    MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_VERSION,
    MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFSTARTUP_NOSOCKET, MFStartup,
    MFT_CATEGORY_VIDEO_DECODER, MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG_HARDWARE,
    MFT_ENUM_FLAG_SORTANDFILTER, MFT_ENUM_FLAG_SYNCMFT, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
    MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_OUTPUT_DATA_BUFFER, MFT_OUTPUT_STREAM_INFO, MFTEnumEx,
    MFVideoInterlace_Progressive,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::core::GUID;

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

/// Initialize Media Foundation (call once at startup).
fn init_mf() -> Result<(), CodecError> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)
            .map_err(|e| CodecError::InitializationFailed(format!("MFStartup failed: {e}")))?;
    }
    Ok(())
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

unsafe impl Send for WindowsDecoder {}
unsafe impl Sync for WindowsDecoder {}

impl WindowsDecoder {
    /// Create a new Windows hardware decoder.
    pub fn new(
        codec_type: CodecType,
        _config: Option<&[u8]>,
        width: u32,
        height: u32,
    ) -> Result<Self, CodecError> {
        init_mf()?;

        unsafe {
            let input_type = create_video_type(codec_type.subtype(), width, height)?;
            let output_type = create_video_type(MFVideoFormat_NV12, width, height)?;

            let mut count = 0u32;
            let mut activates = ptr::null_mut();

            MFTEnumEx(
                MFT_CATEGORY_VIDEO_DECODER,
                MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER | MFT_ENUM_FLAG_SYNCMFT,
                Some(&create_register_type_info(codec_type.subtype())),
                Some(&create_register_type_info(MFVideoFormat_NV12)),
                &raw mut activates,
                &raw mut count,
            )
            .map_err(|e| {
                CodecError::InitializationFailed(format!("MFTEnumEx decoder failed: {e}"))
            })?;

            if count == 0 || activates.is_null() {
                return Err(CodecError::Unsupported(
                    "No hardware decoder available".into(),
                ));
            }

            let activate = (&*activates)
                .as_ref()
                .ok_or_else(|| CodecError::InitializationFailed("No decoder found".into()))?;
            let transform: IMFTransform = activate
                .ActivateObject()
                .map_err(|e| CodecError::InitializationFailed(format!("ActivateObject: {e}")))?;

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
            })
        }
    }

    /// Decode compressed video data.
    pub fn decode(&mut self, data: &[u8]) -> Result<Vec<WindowsFrame>, CodecError> {
        unsafe {
            let input_sample = create_sample(data)?;

            self.transform
                .ProcessInput(0, &input_sample, 0)
                .map_err(|e| CodecError::DecodingFailed(format!("ProcessInput: {e}")))?;

            let mut frames = Vec::new();

            loop {
                let output_sample = if self.output_stream_info.dwFlags & 0x100 != 0 {
                    None
                } else {
                    let y_size = self.width as usize * self.height as usize;
                    let uv_size = y_size / 2;
                    Some(create_empty_sample(y_size + uv_size)?)
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

                match result {
                    Ok(()) => {
                        if let Some(sample) = &*output_buffer.pSample
                            && let Ok(frame) = extract_nv12_frame(sample, self.width, self.height)
                        {
                            frames.push(frame);
                        }
                    }
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => break,
                    Err(e) => {
                        return Err(CodecError::DecodingFailed(format!("ProcessOutput: {e}")));
                    }
                }
            }

            Ok(frames)
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

unsafe impl Send for WindowsEncoder {}
unsafe impl Sync for WindowsEncoder {}

impl WindowsEncoder {
    /// Create a new Windows hardware encoder.
    pub fn new(codec_type: CodecType, width: u32, height: u32) -> Result<Self, CodecError> {
        init_mf()?;

        unsafe {
            let mut count = 0u32;
            let mut activates = ptr::null_mut();

            MFTEnumEx(
                MFT_CATEGORY_VIDEO_ENCODER,
                MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER | MFT_ENUM_FLAG_SYNCMFT,
                Some(&create_register_type_info(MFVideoFormat_NV12)),
                Some(&create_register_type_info(codec_type.subtype())),
                &raw mut activates,
                &raw mut count,
            )
            .map_err(|e| {
                CodecError::InitializationFailed(format!("MFTEnumEx encoder failed: {e}"))
            })?;

            if count == 0 || activates.is_null() {
                return Err(CodecError::Unsupported(
                    "No hardware encoder available".into(),
                ));
            }

            let activate = (&*activates)
                .as_ref()
                .ok_or_else(|| CodecError::InitializationFailed("No encoder found".into()))?;
            let transform: IMFTransform = activate
                .ActivateObject()
                .map_err(|e| CodecError::InitializationFailed(format!("ActivateObject: {e}")))?;

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

                match result {
                    Ok(()) => {
                        if let Some(sample) = &*output_buffer.pSample
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
            .SetGUID(&MF_MT_SUBTYPE, &subtype)
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

fn create_register_type_info(
    subtype: GUID,
) -> windows::Win32::Media::MediaFoundation::MFT_REGISTER_TYPE_INFO {
    windows::Win32::Media::MediaFoundation::MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: subtype,
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
fn extract_nv12_frame(
    sample: &IMFSample,
    width: u32,
    height: u32,
) -> Result<WindowsFrame, CodecError> {
    unsafe {
        let buffer = sample
            .GetBufferByIndex(0)
            .map_err(|e| CodecError::DecodingFailed(format!("GetBufferByIndex: {e}")))?;

        let mut buffer_ptr = ptr::null_mut();
        let mut current_len = 0u32;

        buffer
            .Lock(&raw mut buffer_ptr, None, Some(&raw mut current_len))
            .map_err(|e| CodecError::DecodingFailed(format!("Lock: {e}")))?;

        let y_size = width as usize * height as usize;
        let uv_size = y_size / 2;
        let frame_size = y_size + uv_size;

        let mut data = vec![0u8; frame_size];
        let copy_size = (current_len as usize).min(frame_size);
        ptr::copy_nonoverlapping(buffer_ptr, data.as_mut_ptr(), copy_size);

        buffer
            .Unlock()
            .map_err(|e| CodecError::DecodingFailed(format!("Unlock: {e}")))?;

        let timestamp_ns = sample.GetSampleTime().unwrap_or(0) as u64 * 100;

        Ok(WindowsFrame {
            data,
            width,
            height,
            timestamp_ns,
        })
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
