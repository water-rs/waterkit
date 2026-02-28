//! Linux VA-API codec backend (non-FFmpeg path).
//!
//! Uses `libva` directly for capability probing and `cros-codecs` VA-API worker
//! wrappers for decode/encode pipelines.

use crate::{CodecError, HdrSupport, SupportLevel, VaapiEncodeSupport};
use cros_codecs::c2_wrapper::c2_decoder::C2DecoderWorker;
use cros_codecs::c2_wrapper::c2_encoder::C2EncoderWorker;
use cros_codecs::c2_wrapper::c2_vaapi_decoder::{C2VaapiDecoder, C2VaapiDecoderOptions};
use cros_codecs::c2_wrapper::c2_vaapi_encoder::{C2VaapiEncoder, C2VaapiEncoderOptions};
use cros_codecs::c2_wrapper::{C2DecodeJob, C2EncodeJob, C2Status, C2Wrapper, DrainMode};
use cros_codecs::libva::{Display, VAEntrypoint, VAProfile};
use cros_codecs::video_frame::VideoFrame;
use cros_codecs::video_frame::gbm_video_frame::{GbmDevice, GbmUsage, GbmVideoFrame};
use cros_codecs::{Fourcc, Resolution};
use std::collections::VecDeque;
use std::fmt;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_BITRATE: u64 = 4_000_000;
const DEFAULT_FRAMERATE: u32 = 30;

const FRAME_WAIT_TIMEOUT: Duration = Duration::from_millis(50);
const ENCODE_WAIT_TIMEOUT: Duration = Duration::from_millis(120);

const RENDER_NODE_DIR: &str = "/dev/dri";
const RENDER_NODE_PREFIX: &str = "renderD";

type VaProfileType = VAProfile::Type;
type VaEntrypointType = VAEntrypoint::Type;

type DecodeWrapper =
    C2Wrapper<C2DecodeJob<GbmVideoFrame>, C2DecoderWorker<GbmVideoFrame, C2VaapiDecoder>>;
type EncodeWrapper =
    C2Wrapper<C2EncodeJob<GbmVideoFrame>, C2EncoderWorker<GbmVideoFrame, C2VaapiEncoder>>;

const H264_DECODE_PROFILES: &[VaProfileType] = &[
    VAProfile::VAProfileH264ConstrainedBaseline,
    VAProfile::VAProfileH264Baseline,
    VAProfile::VAProfileH264Main,
    VAProfile::VAProfileH264High,
];
const H265_DECODE_PROFILES: &[VaProfileType] =
    &[VAProfile::VAProfileHEVCMain, VAProfile::VAProfileHEVCMain10];
const AV1_DECODE_PROFILES: &[VaProfileType] = &[
    VAProfile::VAProfileAV1Profile0,
    VAProfile::VAProfileAV1Profile1,
];

const H264_ENCODE_PROFILES: &[VaProfileType] = &[
    VAProfile::VAProfileH264ConstrainedBaseline,
    VAProfile::VAProfileH264Main,
    VAProfile::VAProfileH264High,
];
// cros-codecs VA-API encoder currently does not expose H.265.
const H265_ENCODE_PROFILES: &[VaProfileType] = &[];
const AV1_ENCODE_PROFILES: &[VaProfileType] = &[
    VAProfile::VAProfileAV1Profile0,
    VAProfile::VAProfileAV1Profile1,
];

const H265_HDR_PROFILES: &[VaProfileType] = &[VAProfile::VAProfileHEVCMain10];
const AV1_HDR_PROFILES: &[VaProfileType] = &[VAProfile::VAProfileAV1Profile1];

const DECODE_ENTRYPOINTS: &[VaEntrypointType] = &[VAEntrypoint::VAEntrypointVLD];
const ENCODE_ENTRYPOINTS: &[VaEntrypointType] = &[
    VAEntrypoint::VAEntrypointEncSliceLP,
    VAEntrypoint::VAEntrypointEncSlice,
];

/// Internal codec type for Linux implementations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecType {
    H264,
    H265,
    Av1,
}

#[derive(Debug, Clone, Copy, Default)]
struct VaapiCapabilities {
    h264_decode: bool,
    h265_decode: bool,
    av1_decode: bool,
    h264_encode: bool,
    h265_encode: bool,
    av1_encode: bool,
    hdr10_decode: bool,
    hdr10_encode: bool,
}

fn find_render_nodes() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(RENDER_NODE_DIR) else {
        return Vec::new();
    };

    let mut nodes = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(RENDER_NODE_PREFIX))
        })
        .collect::<Vec<_>>();

    nodes.sort();
    nodes
}

fn open_gbm_device() -> Result<Arc<GbmDevice>, CodecError> {
    for node in find_render_nodes() {
        if let Ok(device) = GbmDevice::open(&node) {
            return Ok(device);
        }
    }

    Err(CodecError::InitializationFailed(
        "failed to open GBM device (expected /dev/dri/renderD*)".into(),
    ))
}

fn open_display() -> Result<Rc<Display>, CodecError> {
    Display::open().ok_or_else(|| {
        CodecError::InitializationFailed(
            "failed to open VA-API DRM display (expected /dev/dri/renderD*)".into(),
        )
    })
}

fn supports_profiles(
    display: &Display,
    available_profiles: &[VaProfileType],
    profile_candidates: &[VaProfileType],
    entrypoint_candidates: &[VaEntrypointType],
) -> bool {
    profile_candidates.iter().copied().any(|profile| {
        if !available_profiles.contains(&profile) {
            return false;
        }

        let Ok(entrypoints) = display.query_config_entrypoints(profile) else {
            return false;
        };

        entrypoint_candidates
            .iter()
            .any(|entrypoint| entrypoints.contains(entrypoint))
    })
}

fn query_vaapi_capabilities() -> Result<VaapiCapabilities, CodecError> {
    let display = open_display()?;
    let available_profiles = display.query_config_profiles().map_err(|e| {
        CodecError::InitializationFailed(format!(
            "vaQueryConfigProfiles failed while probing capabilities: {e}"
        ))
    })?;

    Ok(VaapiCapabilities {
        h264_decode: supports_profiles(
            display.as_ref(),
            &available_profiles,
            H264_DECODE_PROFILES,
            DECODE_ENTRYPOINTS,
        ),
        h265_decode: supports_profiles(
            display.as_ref(),
            &available_profiles,
            H265_DECODE_PROFILES,
            DECODE_ENTRYPOINTS,
        ),
        av1_decode: supports_profiles(
            display.as_ref(),
            &available_profiles,
            AV1_DECODE_PROFILES,
            DECODE_ENTRYPOINTS,
        ),
        h264_encode: supports_profiles(
            display.as_ref(),
            &available_profiles,
            H264_ENCODE_PROFILES,
            ENCODE_ENTRYPOINTS,
        ),
        h265_encode: supports_profiles(
            display.as_ref(),
            &available_profiles,
            H265_ENCODE_PROFILES,
            ENCODE_ENTRYPOINTS,
        ),
        av1_encode: supports_profiles(
            display.as_ref(),
            &available_profiles,
            AV1_ENCODE_PROFILES,
            ENCODE_ENTRYPOINTS,
        ),
        hdr10_decode: supports_profiles(
            display.as_ref(),
            &available_profiles,
            H265_HDR_PROFILES,
            DECODE_ENTRYPOINTS,
        ) || supports_profiles(
            display.as_ref(),
            &available_profiles,
            AV1_HDR_PROFILES,
            DECODE_ENTRYPOINTS,
        ),
        hdr10_encode: supports_profiles(
            display.as_ref(),
            &available_profiles,
            H265_HDR_PROFILES,
            ENCODE_ENTRYPOINTS,
        ) || supports_profiles(
            display.as_ref(),
            &available_profiles,
            AV1_HDR_PROFILES,
            ENCODE_ENTRYPOINTS,
        ),
    })
}

fn decode_supported(caps: &VaapiCapabilities, codec: CodecType) -> bool {
    match codec {
        CodecType::H264 => caps.h264_decode,
        CodecType::H265 => caps.h265_decode,
        CodecType::Av1 => caps.av1_decode,
    }
}

fn encode_supported(caps: &VaapiCapabilities, codec: CodecType) -> bool {
    match codec {
        CodecType::H264 => caps.h264_encode,
        CodecType::H265 => caps.h265_encode,
        CodecType::Av1 => caps.av1_encode,
    }
}

fn encoded_fourcc(codec: CodecType) -> Fourcc {
    match codec {
        CodecType::H264 => Fourcc::from(b"H264"),
        CodecType::H265 => Fourcc::from(b"HEVC"),
        CodecType::Av1 => Fourcc::from(b"AV1F"),
    }
}

fn check_c2_status(status: C2Status, context: &str) -> Result<(), CodecError> {
    if status == C2Status::C2Ok {
        Ok(())
    } else {
        Err(CodecError::InitializationFailed(format!(
            "{context} failed with status {status:?}"
        )))
    }
}

fn copy_plane_tight(
    dst: &mut Vec<u8>,
    src_plane: &[u8],
    src_stride: usize,
    row_bytes: usize,
    rows: usize,
) {
    for row in 0..rows {
        let start = row * src_stride;
        let end = start.saturating_add(row_bytes).min(src_plane.len());
        if end > start {
            dst.extend_from_slice(&src_plane[start..end]);
            if end - start < row_bytes {
                dst.resize(dst.len() + (row_bytes - (end - start)), 0);
            }
        } else {
            dst.resize(dst.len() + row_bytes, 0);
        }
    }
}

fn map_frame_to_nv12(frame: &GbmVideoFrame) -> Result<Vec<u8>, CodecError> {
    let resolution = frame.resolution();
    let width = resolution.width as usize;
    let height = resolution.height as usize;
    let y_size = width * height;
    let uv_size = y_size / 2;

    let mapping = frame
        .map()
        .map_err(|e| CodecError::DecodingFailed(format!("failed to map decoded frame: {e}")))?;
    let planes = mapping.get();
    let strides = frame.get_plane_pitch();

    if planes.len() < 2 || strides.len() < 2 {
        return Err(CodecError::DecodingFailed(
            "decoded frame missing NV12 planes".into(),
        ));
    }

    let mut out = Vec::with_capacity(y_size + uv_size);
    copy_plane_tight(&mut out, planes[0], strides[0], width, height);
    copy_plane_tight(&mut out, planes[1], strides[1], width, height / 2);
    Ok(out)
}

fn write_nv12_to_gbm(frame: &mut GbmVideoFrame, nv12: &[u8]) -> Result<(), CodecError> {
    let resolution = frame.resolution();
    let width = resolution.width as usize;
    let height = resolution.height as usize;

    let y_size = width * height;
    let uv_size = y_size / 2;
    let expected = y_size + uv_size;
    if nv12.len() != expected {
        return Err(CodecError::EncodingFailed(format!(
            "NV12 data size {} doesn't match expected {} for {}x{}",
            nv12.len(),
            expected,
            resolution.width,
            resolution.height
        )));
    }

    let strides = frame.get_plane_pitch().to_vec();
    let mapping = frame
        .map_mut()
        .map_err(|e| CodecError::EncodingFailed(format!("failed to map encoder input: {e}")))?;
    let planes = mapping.get();

    if planes.len() < 2 || strides.len() < 2 {
        return Err(CodecError::EncodingFailed(
            "encoder frame missing NV12 planes".into(),
        ));
    }

    {
        let mut y_plane = planes[0].borrow_mut();
        for row in 0..height {
            let src_start = row * width;
            let src_end = src_start + width;
            let dst_start = row * strides[0];
            let dst_end = dst_start + width;
            if dst_end <= y_plane.len() {
                y_plane[dst_start..dst_end].copy_from_slice(&nv12[src_start..src_end]);
            }
        }
    }

    {
        let mut uv_plane = planes[1].borrow_mut();
        for row in 0..(height / 2) {
            let src_start = y_size + row * width;
            let src_end = src_start + width;
            let dst_start = row * strides[1];
            let dst_end = dst_start + width;
            if dst_end <= uv_plane.len() {
                uv_plane[dst_start..dst_end].copy_from_slice(&nv12[src_start..src_end]);
            }
        }
    }

    Ok(())
}

/// Query VA-API encoder availability for Linux runtime.
#[must_use]
pub fn check_vaapi_encode_support() -> VaapiEncodeSupport {
    let caps = query_vaapi_capabilities().unwrap_or_default();
    VaapiEncodeSupport {
        h264: caps.h264_encode,
        h265: caps.h265_encode,
        av1: caps.av1_encode,
    }
}

/// Query 10-bit HDR support hints for Linux runtime.
#[must_use]
pub fn check_hdr_support() -> HdrSupport {
    let caps = query_vaapi_capabilities().unwrap_or_default();

    HdrSupport {
        decode_10bit: if caps.hdr10_decode {
            SupportLevel::Supported
        } else {
            SupportLevel::Unsupported
        },
        encode_10bit: if caps.hdr10_encode {
            SupportLevel::Supported
        } else {
            SupportLevel::Unsupported
        },
    }
}

/// Decoded frame from Linux VA-API backend (`NV12` format).
#[derive(Clone)]
pub struct LinuxFrame {
    /// `NV12` data: Y plane followed by interleaved UV plane.
    pub data: Vec<u8>,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Presentation timestamp in nanoseconds.
    pub timestamp_ns: u64,
}

impl fmt::Debug for LinuxFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LinuxFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("timestamp_ns", &self.timestamp_ns)
            .finish_non_exhaustive()
    }
}

/// Linux VA-API decoder.
pub struct LinuxDecoder {
    codec_type: CodecType,
    width: u32,
    height: u32,
    decoder: DecodeWrapper,
    output_frames: Arc<Mutex<VecDeque<LinuxFrame>>>,
    worker_error: Arc<Mutex<Option<CodecError>>>,
}

impl fmt::Debug for LinuxDecoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LinuxDecoder")
            .field("codec_type", &self.codec_type)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl LinuxDecoder {
    /// Create a new Linux VA-API decoder.
    pub fn new(
        codec_type: CodecType,
        config: Option<&[u8]>,
        width: u32,
        height: u32,
    ) -> Result<Self, CodecError> {
        let _ = config;

        let caps = query_vaapi_capabilities()?;
        if !decode_supported(&caps, codec_type) {
            return Err(CodecError::Unsupported(format!(
                "VA-API decoder unavailable for {codec_type:?}"
            )));
        }

        let gbm_device = open_gbm_device()?;
        let stream_info = Arc::new(Mutex::new(cros_codecs::decoder::StreamInfo {
            format: cros_codecs::DecodedFormat::NV12,
            coded_resolution: Resolution { width, height },
            display_resolution: Resolution { width, height },
            min_num_frames: 6,
        }));
        let output_frames = Arc::new(Mutex::new(VecDeque::new()));
        let worker_error = Arc::new(Mutex::new(None));

        let output_frames_cb = Arc::clone(&output_frames);
        let worker_error_cb = Arc::clone(&worker_error);
        let work_done_cb = move |job: C2DecodeJob<GbmVideoFrame>| {
            if let Some(frame) = job.output {
                let resolution = frame.resolution();
                match map_frame_to_nv12(frame.as_ref()) {
                    Ok(data) => {
                        output_frames_cb.lock().unwrap().push_back(LinuxFrame {
                            data,
                            width: resolution.width,
                            height: resolution.height,
                            timestamp_ns: 0,
                        });
                    }
                    Err(e) => {
                        *worker_error_cb.lock().unwrap() = Some(e);
                    }
                }
            }
        };

        let worker_error_err = Arc::clone(&worker_error);
        let error_cb = move |status: C2Status| {
            *worker_error_err.lock().unwrap() = Some(CodecError::DecodingFailed(format!(
                "VA-API decoder worker error: {status:?}"
            )));
        };

        let stream_info_hint = Arc::clone(&stream_info);
        let framepool_hint_cb = move |info: cros_codecs::decoder::StreamInfo| {
            *stream_info_hint.lock().unwrap() = info;
        };

        let stream_info_alloc = Arc::clone(&stream_info);
        let gbm_device_alloc = Arc::clone(&gbm_device);
        let alloc_cb = move || {
            let info = stream_info_alloc.lock().unwrap().clone();
            gbm_device_alloc
                .clone()
                .new_frame(
                    Fourcc::from(cros_codecs::DecodedFormat::NV12),
                    info.display_resolution,
                    info.coded_resolution,
                    GbmUsage::Decode,
                )
                .ok()
        };

        let mut decoder: DecodeWrapper = C2Wrapper::new(
            encoded_fourcc(codec_type),
            Fourcc::from(cros_codecs::DecodedFormat::NV12),
            error_cb,
            work_done_cb,
            framepool_hint_cb,
            alloc_cb,
            C2VaapiDecoderOptions {
                libva_device_path: find_render_nodes().into_iter().next(),
            },
        );
        check_c2_status(decoder.start(), "starting VA-API decoder")?;

        Ok(Self {
            codec_type,
            width,
            height,
            decoder,
            output_frames,
            worker_error,
        })
    }

    /// Decode compressed video data.
    pub fn decode(&mut self, data: &[u8]) -> Result<Vec<LinuxFrame>, CodecError> {
        if let Some(err) = self.worker_error.lock().unwrap().take() {
            return Err(err);
        }

        let mut result = Vec::new();
        {
            let mut queue = self.output_frames.lock().unwrap();
            while let Some(frame) = queue.pop_front() {
                result.push(frame);
            }
        }

        if data.is_empty() {
            return Ok(result);
        }

        let status = self.decoder.queue(vec![C2DecodeJob {
            input: data.to_vec(),
            output: None,
            drain: DrainMode::NoDrain,
        }]);
        check_c2_status(status, "queueing decode job")?;

        let start = Instant::now();
        while start.elapsed() < FRAME_WAIT_TIMEOUT {
            if self.worker_error.lock().unwrap().is_some() {
                break;
            }
            if !self.output_frames.lock().unwrap().is_empty() {
                break;
            }
            if !self.decoder.is_alive() {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }

        if let Some(err) = self.worker_error.lock().unwrap().take() {
            return Err(err);
        }

        let mut queue = self.output_frames.lock().unwrap();
        while let Some(frame) = queue.pop_front() {
            result.push(frame);
        }

        Ok(result)
    }
}

impl Drop for LinuxDecoder {
    fn drop(&mut self) {
        if self.decoder.is_alive() {
            let _ = self.decoder.drain(DrainMode::EOSDrain);
        }
    }
}

/// Linux VA-API encoder.
pub struct LinuxEncoder {
    codec_type: CodecType,
    width: u32,
    height: u32,
    encoder: EncodeWrapper,
    output_packets: Arc<Mutex<VecDeque<Vec<u8>>>>,
    worker_error: Arc<Mutex<Option<CodecError>>>,
    gbm_device: Arc<GbmDevice>,
    frame_index: u64,
    codec_config: Option<Vec<u8>>,
}

impl fmt::Debug for LinuxEncoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LinuxEncoder")
            .field("codec_type", &self.codec_type)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("frame_index", &self.frame_index)
            .finish_non_exhaustive()
    }
}

impl LinuxEncoder {
    /// Create a new Linux VA-API encoder.
    pub fn new(codec_type: CodecType, width: u32, height: u32) -> Result<Self, CodecError> {
        let caps = query_vaapi_capabilities()?;
        if !encode_supported(&caps, codec_type) {
            return Err(CodecError::Unsupported(format!(
                "VA-API encoder unavailable for {codec_type:?}"
            )));
        }

        // cros-codecs VA-API encoder currently supports H264/AV1 (and VP9), not H265.
        if matches!(codec_type, CodecType::H265) {
            return Err(CodecError::Unsupported(
                "H265 VA-API encode is not available in current Linux backend".into(),
            ));
        }

        let gbm_device = open_gbm_device()?;
        let stream_info = Arc::new(Mutex::new(cros_codecs::decoder::StreamInfo {
            format: cros_codecs::DecodedFormat::NV12,
            coded_resolution: Resolution { width, height },
            display_resolution: Resolution { width, height },
            min_num_frames: 0,
        }));

        let output_packets = Arc::new(Mutex::new(VecDeque::new()));
        let worker_error = Arc::new(Mutex::new(None));

        let output_packets_cb = Arc::clone(&output_packets);
        let work_done_cb = move |job: C2EncodeJob<GbmVideoFrame>| {
            if !job.output.is_empty() {
                output_packets_cb.lock().unwrap().push_back(job.output);
            }
        };

        let worker_error_err = Arc::clone(&worker_error);
        let error_cb = move |status: C2Status| {
            *worker_error_err.lock().unwrap() = Some(CodecError::EncodingFailed(format!(
                "VA-API encoder worker error: {status:?}"
            )));
        };

        let stream_info_hint = Arc::clone(&stream_info);
        let framepool_hint_cb = move |info: cros_codecs::decoder::StreamInfo| {
            *stream_info_hint.lock().unwrap() = info;
        };

        let stream_info_alloc = Arc::clone(&stream_info);
        let gbm_device_alloc = Arc::clone(&gbm_device);
        let alloc_cb = move || {
            let info = stream_info_alloc.lock().unwrap().clone();
            gbm_device_alloc
                .clone()
                .new_frame(
                    Fourcc::from(cros_codecs::DecodedFormat::NV12),
                    info.display_resolution,
                    info.coded_resolution,
                    GbmUsage::Encode,
                )
                .ok()
        };

        let mut encoder: EncodeWrapper = C2Wrapper::new(
            Fourcc::from(cros_codecs::DecodedFormat::NV12),
            encoded_fourcc(codec_type),
            error_cb,
            work_done_cb,
            framepool_hint_cb,
            alloc_cb,
            C2VaapiEncoderOptions {
                low_power: false,
                visible_resolution: Resolution { width, height },
            },
        );
        check_c2_status(encoder.start(), "starting VA-API encoder")?;

        Ok(Self {
            codec_type,
            width,
            height,
            encoder,
            output_packets,
            worker_error,
            gbm_device,
            frame_index: 0,
            codec_config: None,
        })
    }

    /// Encode `NV12` data to compressed video.
    pub fn encode_nv12(&mut self, nv12: &[u8]) -> Result<Vec<u8>, CodecError> {
        if let Some(err) = self.worker_error.lock().unwrap().take() {
            return Err(err);
        }

        let resolution = Resolution {
            width: self.width,
            height: self.height,
        };

        let mut input_frame = self
            .gbm_device
            .clone()
            .new_frame(
                Fourcc::from(cros_codecs::DecodedFormat::NV12),
                resolution,
                resolution,
                GbmUsage::Encode,
            )
            .map_err(|e| {
                CodecError::EncodingFailed(format!("failed to allocate encode frame: {e}"))
            })?;

        write_nv12_to_gbm(&mut input_frame, nv12)?;

        let timestamp_us = (self.frame_index * 1_000_000) / (DEFAULT_FRAMERATE as u64);
        self.frame_index += 1;

        let status = self.encoder.queue(vec![C2EncodeJob {
            input: Some(input_frame),
            output: Vec::new(),
            timestamp: timestamp_us,
            bitrate: DEFAULT_BITRATE,
            framerate: Arc::new(AtomicU32::new(DEFAULT_FRAMERATE)),
            drain: DrainMode::NoDrain,
        }]);
        check_c2_status(status, "queueing encode job")?;

        let start = Instant::now();
        while start.elapsed() < ENCODE_WAIT_TIMEOUT {
            if self.worker_error.lock().unwrap().is_some() {
                break;
            }
            if !self.output_packets.lock().unwrap().is_empty() {
                break;
            }
            if !self.encoder.is_alive() {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }

        if let Some(err) = self.worker_error.lock().unwrap().take() {
            return Err(err);
        }

        let mut output = Vec::new();
        let mut packets = self.output_packets.lock().unwrap();
        while let Some(packet) = packets.pop_front() {
            if self.codec_config.is_none() && !packet.is_empty() {
                self.codec_config = Some(packet.clone());
            }
            output.extend_from_slice(&packet);
        }

        Ok(output)
    }

    /// Get the codec configuration data if available.
    #[must_use]
    pub fn get_codec_config(&self) -> Option<Vec<u8>> {
        self.codec_config.clone()
    }
}

impl Drop for LinuxEncoder {
    fn drop(&mut self) {
        if self.encoder.is_alive() {
            let _ = self.encoder.drain(DrainMode::EOSDrain);
        }
    }
}
