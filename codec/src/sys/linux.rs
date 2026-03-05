//! Linux VA-API hardware encoding and decoding.
//!
//! This backend uses pure VA-API via `cros-codecs`.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::missing_const_for_fn,
    clippy::uninlined_format_args
)]

use crate::CodecError;
use cros_codecs::decoder::stateless::h264::H264;
use cros_codecs::decoder::stateless::h265::H265;
use cros_codecs::decoder::stateless::{
    DecodeError, DynStatelessVideoDecoder, StatelessDecoder, StatelessVideoDecoder,
};
use cros_codecs::decoder::{DecodedHandle, DecoderEvent};
use cros_codecs::encoder::h264::EncoderConfig as H264EncoderConfig;
use cros_codecs::encoder::stateless::h264;
use cros_codecs::encoder::{FrameMetadata, VideoEncoder};
use cros_codecs::video_frame::VideoFrame;
use cros_codecs::video_frame::gbm_video_frame::{GbmDevice, GbmUsage, GbmVideoFrame};
use cros_codecs::video_frame::generic_dma_video_frame::GenericDmaVideoFrame;
use cros_codecs::{BlockingMode, Fourcc, FrameLayout, Resolution};
use std::fmt;
use std::sync::Arc;

const RENDER_NODE_PATH: &str = "/dev/dri/renderD128";
const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// Internal codec type for Linux implementations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecType {
    H264,
    H265,
}

/// Decoded frame from Linux VA-API (`NV12` format).
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

enum InputBitstream {
    AnnexB,
    LengthPrefixed(LengthPrefixedAnnexB),
}

struct LengthPrefixedAnnexB {
    nal_length_size: usize,
    prefix_nalus: Vec<u8>,
    sent_prefix: bool,
}

impl LengthPrefixedAnnexB {
    fn convert_sample(&mut self, sample: &[u8]) -> Result<Vec<u8>, CodecError> {
        let mut annex_b = length_prefixed_to_annex_b(sample, self.nal_length_size)?;
        if !self.sent_prefix {
            self.sent_prefix = true;
            if self.prefix_nalus.is_empty() {
                return Ok(annex_b);
            }
            let mut prefixed = Vec::with_capacity(self.prefix_nalus.len() + annex_b.len());
            prefixed.extend_from_slice(&self.prefix_nalus);
            prefixed.append(&mut annex_b);
            return Ok(prefixed);
        }
        Ok(annex_b)
    }
}

/// Linux VA-API decoder.
pub struct LinuxDecoder {
    decoder: DynStatelessVideoDecoder<GenericDmaVideoFrame>,
    gbm_device: Arc<GbmDevice>,
    codec_type: CodecType,
    coded_resolution: Resolution,
    display_resolution: Resolution,
    next_timestamp: u64,
    input_bitstream: InputBitstream,
}

impl fmt::Debug for LinuxDecoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LinuxDecoder")
            .field("codec_type", &self.codec_type)
            .field("coded_resolution", &self.coded_resolution)
            .field("display_resolution", &self.display_resolution)
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
        let display = cros_codecs::libva::Display::open().ok_or_else(|| {
            CodecError::InitializationFailed("failed to open VA-API display".to_string())
        })?;

        let decoder: DynStatelessVideoDecoder<GenericDmaVideoFrame> = match codec_type {
            CodecType::H264 => {
                StatelessDecoder::<H264, _>::new_vaapi(display, BlockingMode::NonBlocking)
                    .map_err(|e| {
                        CodecError::InitializationFailed(format!(
                            "failed to create VA-API H264 decoder: {e}"
                        ))
                    })?
                    .into_trait_object()
            }
            CodecType::H265 => {
                StatelessDecoder::<H265, _>::new_vaapi(display, BlockingMode::NonBlocking)
                    .map_err(|e| {
                        CodecError::InitializationFailed(format!(
                            "failed to create VA-API H265 decoder: {e}"
                        ))
                    })?
                    .into_trait_object()
            }
        };

        let input_bitstream = decode_bitstream_mode(codec_type, config)?;
        let gbm_device = GbmDevice::open(RENDER_NODE_PATH).map_err(|e| {
            CodecError::InitializationFailed(format!("failed to open GBM render node: {e}"))
        })?;

        Ok(Self {
            decoder,
            gbm_device,
            codec_type,
            coded_resolution: Resolution { width, height },
            display_resolution: Resolution { width, height },
            next_timestamp: 0,
            input_bitstream,
        })
    }

    /// Decode compressed video data.
    pub fn decode(&mut self, data: &[u8]) -> Result<Vec<LinuxFrame>, CodecError> {
        let packet = self.prepare_annex_b_packet(data)?;
        if packet.is_empty() {
            return Ok(Vec::new());
        }

        let mut frames = Vec::new();
        let mut offset = 0;

        while offset < packet.len() {
            let gbm_device = Arc::clone(&self.gbm_device);
            let display_resolution = self.display_resolution;
            let coded_resolution = self.coded_resolution;
            let mut allocate_frame = || {
                Some(Self::allocate_decode_frame(
                    &gbm_device,
                    display_resolution,
                    coded_resolution,
                ))
            };
            match self
                .decoder
                .decode(self.next_timestamp, &packet[offset..], &mut allocate_frame)
            {
                Ok(consumed) => {
                    if consumed == 0 {
                        return Err(CodecError::DecodingFailed(
                            "VA-API decoder consumed 0 bytes".to_string(),
                        ));
                    }
                    offset += consumed;
                    self.next_timestamp += 1;
                    self.collect_decoder_events(&mut frames)?;
                }
                Err(DecodeError::NotEnoughOutputBuffers(_) | DecodeError::CheckEvents) => {
                    self.collect_decoder_events(&mut frames)?;
                }
                Err(e) => {
                    return Err(CodecError::DecodingFailed(format!(
                        "VA-API decode failed: {e}"
                    )));
                }
            }
        }

        self.collect_decoder_events(&mut frames)?;

        Ok(frames)
    }

    fn prepare_annex_b_packet(&mut self, data: &[u8]) -> Result<Vec<u8>, CodecError> {
        match &mut self.input_bitstream {
            InputBitstream::AnnexB => Ok(data.to_vec()),
            InputBitstream::LengthPrefixed(state) => state.convert_sample(data),
        }
    }

    fn allocate_decode_frame(
        gbm_device: &Arc<GbmDevice>,
        display_resolution: Resolution,
        coded_resolution: Resolution,
    ) -> GenericDmaVideoFrame {
        Arc::clone(gbm_device)
            .new_frame(
                Fourcc::from(b"NV12"),
                display_resolution,
                coded_resolution,
                GbmUsage::Decode,
            )
            .and_then(GbmVideoFrame::to_generic_dma_video_frame)
            .expect("failed to allocate VA-API decode output frame")
    }

    fn collect_decoder_events(&mut self, frames: &mut Vec<LinuxFrame>) -> Result<(), CodecError> {
        loop {
            match self.decoder.next_event() {
                Some(DecoderEvent::FormatChanged) => {
                    let stream_info = self.decoder.stream_info().ok_or_else(|| {
                        CodecError::DecodingFailed(
                            "decoder emitted FormatChanged without stream info".to_string(),
                        )
                    })?;
                    self.coded_resolution = stream_info.coded_resolution;
                    self.display_resolution = stream_info.display_resolution;
                }
                Some(DecoderEvent::FrameReady(handle)) => {
                    handle.sync().map_err(|e| {
                        CodecError::DecodingFailed(format!("failed to sync decoded frame: {e}"))
                    })?;

                    let display_resolution = handle.display_resolution();
                    let width = display_resolution.width;
                    let height = display_resolution.height;
                    let timestamp_ns = handle.timestamp();

                    let frame = handle.video_frame();
                    let data = copy_nv12_from_frame(frame.as_ref(), width, height)?;

                    frames.push(LinuxFrame {
                        data,
                        width,
                        height,
                        timestamp_ns,
                    });
                }
                None => return Ok(()),
            }
        }
    }
}

struct LinuxH264Encoder {
    encoder: Box<dyn VideoEncoder<GenericDmaVideoFrame>>,
    gbm_device: Arc<GbmDevice>,
    display_resolution: Resolution,
    coded_resolution: Resolution,
}

/// Linux VA-API encoder.
pub struct LinuxEncoder {
    codec_type: CodecType,
    width: u32,
    height: u32,
    frame_count: u64,
    codec_config: Option<Vec<u8>>,
    h264: Option<LinuxH264Encoder>,
}

impl fmt::Debug for LinuxEncoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LinuxEncoder")
            .field("codec_type", &self.codec_type)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("frame_count", &self.frame_count)
            .finish_non_exhaustive()
    }
}

impl LinuxEncoder {
    /// Create a new Linux VA-API encoder.
    pub fn new(codec_type: CodecType, width: u32, height: u32) -> Result<Self, CodecError> {
        let display_resolution = Resolution { width, height };
        let coded_resolution = Resolution {
            width: align_16(width),
            height: align_16(height),
        };

        let h264 = match codec_type {
            CodecType::H264 => {
                let display = cros_codecs::libva::Display::open().ok_or_else(|| {
                    CodecError::InitializationFailed("failed to open VA-API display".to_string())
                })?;

                let encoder = h264::StatelessEncoder::<GenericDmaVideoFrame, _>::new_vaapi(
                    display,
                    H264EncoderConfig {
                        resolution: display_resolution,
                        ..Default::default()
                    },
                    Fourcc::from(b"NV12"),
                    coded_resolution,
                    false,
                    BlockingMode::Blocking,
                )
                .map_err(|e| {
                    CodecError::InitializationFailed(format!(
                        "failed to create VA-API H264 encoder: {e}"
                    ))
                })?;

                let gbm_device = GbmDevice::open(RENDER_NODE_PATH).map_err(|e| {
                    CodecError::InitializationFailed(format!("failed to open GBM render node: {e}"))
                })?;

                Some(LinuxH264Encoder {
                    encoder: Box::new(encoder),
                    gbm_device,
                    display_resolution,
                    coded_resolution,
                })
            }
            CodecType::H265 => {
                return Err(CodecError::InitializationFailed(
                    "VA-API H265 encoding is not implemented".to_string(),
                ));
            }
        };

        Ok(Self {
            codec_type,
            width,
            height,
            frame_count: 0,
            codec_config: None,
            h264,
        })
    }

    /// Encode `NV12` data to compressed video.
    pub fn encode_nv12(&mut self, nv12: &[u8]) -> Result<Vec<u8>, CodecError> {
        let y_size = (self.width as usize) * (self.height as usize);
        let expected_size = y_size + (y_size / 2);
        if nv12.len() != expected_size {
            return Err(CodecError::EncodingFailed(format!(
                "NV12 size mismatch: got {}, expected {} for {}x{}",
                nv12.len(),
                expected_size,
                self.width,
                self.height
            )));
        }

        let h264 = self.h264.as_mut().ok_or_else(|| {
            CodecError::EncodingFailed("VA-API H264 encoder is not initialized".to_string())
        })?;

        let mut frame = allocate_encode_frame(
            &h264.gbm_device,
            h264.display_resolution,
            h264.coded_resolution,
            GbmUsage::Encode,
        )?;

        write_nv12_into_frame(
            nv12,
            &mut frame,
            h264.display_resolution,
            h264.coded_resolution,
        )?;

        let metadata = FrameMetadata {
            timestamp: self.frame_count,
            layout: FrameLayout::default(),
            force_keyframe: false,
        };

        h264.encoder
            .encode(metadata, frame)
            .map_err(|e| CodecError::EncodingFailed(format!("VA-API encode failed: {e}")))?;

        self.frame_count += 1;

        let mut packet = Vec::new();
        while let Some(coded) = h264
            .encoder
            .poll()
            .map_err(|e| CodecError::EncodingFailed(format!("VA-API poll failed: {e}")))?
        {
            if self.codec_config.is_none() {
                self.codec_config = build_h264_avcc_from_annex_b(&coded.bitstream);
            }
            packet.extend_from_slice(&coded.bitstream);
        }

        Ok(packet)
    }

    /// Get codec configuration data if available.
    #[must_use]
    pub fn get_codec_config(&self) -> Option<Vec<u8>> {
        self.codec_config.clone()
    }
}

fn decode_bitstream_mode(
    codec: CodecType,
    config: Option<&[u8]>,
) -> Result<InputBitstream, CodecError> {
    let Some(config) = config else {
        return Ok(InputBitstream::AnnexB);
    };

    match codec {
        CodecType::H264 => {
            let Some(payload) = avcc_payload(config) else {
                return Ok(InputBitstream::AnnexB);
            };
            let (nal_length_size, prefix_nalus) = parse_h264_avcc(payload)?;
            Ok(InputBitstream::LengthPrefixed(LengthPrefixedAnnexB {
                nal_length_size,
                prefix_nalus,
                sent_prefix: false,
            }))
        }
        CodecType::H265 => {
            let Some(payload) = hvcc_payload(config) else {
                return Ok(InputBitstream::AnnexB);
            };
            let (nal_length_size, prefix_nalus) = parse_h265_hvcc(payload)?;
            Ok(InputBitstream::LengthPrefixed(LengthPrefixedAnnexB {
                nal_length_size,
                prefix_nalus,
                sent_prefix: false,
            }))
        }
    }
}

fn avcc_payload(config: &[u8]) -> Option<&[u8]> {
    if config.len() >= 8 && &config[4..8] == b"avcC" {
        return Some(&config[8..]);
    }
    if config.len() >= 7 && config[0] == 1 {
        return Some(config);
    }
    None
}

fn hvcc_payload(config: &[u8]) -> Option<&[u8]> {
    if config.len() >= 8 && &config[4..8] == b"hvcC" {
        return Some(&config[8..]);
    }
    if config.len() >= 23 && config[0] == 1 {
        return Some(config);
    }
    None
}

fn parse_h264_avcc(payload: &[u8]) -> Result<(usize, Vec<u8>), CodecError> {
    if payload.len() < 7 {
        return Err(CodecError::InitializationFailed(
            "invalid avcC payload: too short".to_string(),
        ));
    }

    let nal_length_size = ((payload[4] & 0x03) + 1) as usize;
    if !(1..=4).contains(&nal_length_size) {
        return Err(CodecError::InitializationFailed(format!(
            "invalid avcC NAL length size: {nal_length_size}"
        )));
    }

    let mut cursor = 6;
    let num_sps = (payload[5] & 0x1f) as usize;
    let mut prefix_nalus = Vec::new();

    for _ in 0..num_sps {
        let nal = read_u16_len_nal(payload, &mut cursor, "SPS")?;
        prefix_nalus.extend_from_slice(&START_CODE);
        prefix_nalus.extend_from_slice(nal);
    }

    if cursor >= payload.len() {
        return Err(CodecError::InitializationFailed(
            "invalid avcC payload: missing PPS count".to_string(),
        ));
    }

    let pps_count = payload[cursor] as usize;
    cursor += 1;
    for _ in 0..pps_count {
        let nal = read_u16_len_nal(payload, &mut cursor, "PPS")?;
        prefix_nalus.extend_from_slice(&START_CODE);
        prefix_nalus.extend_from_slice(nal);
    }

    Ok((nal_length_size, prefix_nalus))
}

fn parse_h265_hvcc(payload: &[u8]) -> Result<(usize, Vec<u8>), CodecError> {
    if payload.len() < 23 {
        return Err(CodecError::InitializationFailed(
            "invalid hvcC payload: too short".to_string(),
        ));
    }

    let nal_length_size = ((payload[21] & 0x03) + 1) as usize;
    if !(1..=4).contains(&nal_length_size) {
        return Err(CodecError::InitializationFailed(format!(
            "invalid hvcC NAL length size: {nal_length_size}"
        )));
    }

    let mut cursor = 23;
    let num_arrays = payload[22] as usize;
    let mut prefix_nalus = Vec::new();

    for _ in 0..num_arrays {
        if cursor + 3 > payload.len() {
            return Err(CodecError::InitializationFailed(
                "invalid hvcC payload: truncated NAL array header".to_string(),
            ));
        }
        cursor += 1;
        let num_nalus = u16::from_be_bytes([payload[cursor], payload[cursor + 1]]) as usize;
        cursor += 2;

        for _ in 0..num_nalus {
            let nal = read_u16_len_nal(payload, &mut cursor, "HEVC NAL")?;
            prefix_nalus.extend_from_slice(&START_CODE);
            prefix_nalus.extend_from_slice(nal);
        }
    }

    Ok((nal_length_size, prefix_nalus))
}

fn read_u16_len_nal<'a>(
    data: &'a [u8],
    cursor: &mut usize,
    label: &str,
) -> Result<&'a [u8], CodecError> {
    if *cursor + 2 > data.len() {
        return Err(CodecError::InitializationFailed(format!(
            "invalid config payload: missing {label} length"
        )));
    }
    let len = u16::from_be_bytes([data[*cursor], data[*cursor + 1]]) as usize;
    *cursor += 2;
    if *cursor + len > data.len() {
        return Err(CodecError::InitializationFailed(format!(
            "invalid config payload: truncated {label}"
        )));
    }
    let nal = &data[*cursor..*cursor + len];
    *cursor += len;
    Ok(nal)
}

fn length_prefixed_to_annex_b(
    sample: &[u8],
    nal_length_size: usize,
) -> Result<Vec<u8>, CodecError> {
    let mut offset = 0;
    let mut out = Vec::with_capacity(sample.len() + 64);

    while offset + nal_length_size <= sample.len() {
        let nal_len = read_length_field(&sample[offset..offset + nal_length_size]);
        offset += nal_length_size;

        if nal_len == 0 {
            continue;
        }
        if offset + nal_len > sample.len() {
            return Err(CodecError::DecodingFailed(
                "length-prefixed sample has truncated NAL unit".to_string(),
            ));
        }

        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(&sample[offset..offset + nal_len]);
        offset += nal_len;
    }

    if offset != sample.len() {
        return Err(CodecError::DecodingFailed(
            "length-prefixed sample has trailing bytes".to_string(),
        ));
    }

    Ok(out)
}

fn read_length_field(bytes: &[u8]) -> usize {
    match bytes.len() {
        1 => bytes[0] as usize,
        2 => u16::from_be_bytes([bytes[0], bytes[1]]) as usize,
        3 => ((bytes[0] as usize) << 16) | ((bytes[1] as usize) << 8) | (bytes[2] as usize),
        4 => u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize,
        _ => panic!("invalid NAL length field size: {}", bytes.len()),
    }
}

fn copy_nv12_from_frame(
    frame: &GenericDmaVideoFrame,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, CodecError> {
    let pitches = frame.get_plane_pitch();
    if pitches.len() < 2 {
        return Err(CodecError::DecodingFailed(
            "decoded frame does not have NV12 pitches".to_string(),
        ));
    }

    let mapping = frame
        .map()
        .map_err(|e| CodecError::DecodingFailed(format!("failed to map frame: {e}")))?;
    let planes = mapping.get();
    if planes.len() < 2 {
        return Err(CodecError::DecodingFailed(
            "decoded frame does not have NV12 planes".to_string(),
        ));
    }

    let y_size = (width as usize) * (height as usize);
    let uv_height = (height as usize) / 2;
    let uv_size = (width as usize) * uv_height;
    let mut out = Vec::with_capacity(y_size + uv_size);

    copy_plane_rows(
        planes[0],
        pitches[0],
        width as usize,
        height as usize,
        &mut out,
        "Y",
    )?;
    copy_plane_rows(
        planes[1],
        pitches[1],
        width as usize,
        uv_height,
        &mut out,
        "UV",
    )?;

    Ok(out)
}

fn copy_plane_rows(
    plane: &[u8],
    pitch: usize,
    row_bytes: usize,
    rows: usize,
    out: &mut Vec<u8>,
    label: &str,
) -> Result<(), CodecError> {
    for row in 0..rows {
        let start = row * pitch;
        let end = start + row_bytes;
        if end > plane.len() {
            return Err(CodecError::DecodingFailed(format!(
                "{label} plane is smaller than declared pitch/size"
            )));
        }
        out.extend_from_slice(&plane[start..end]);
    }
    Ok(())
}

fn allocate_encode_frame(
    gbm_device: &Arc<GbmDevice>,
    display_resolution: Resolution,
    coded_resolution: Resolution,
    usage: GbmUsage,
) -> Result<GenericDmaVideoFrame, CodecError> {
    Arc::clone(gbm_device)
        .new_frame(
            Fourcc::from(b"NV12"),
            display_resolution,
            coded_resolution,
            usage,
        )
        .and_then(GbmVideoFrame::to_generic_dma_video_frame)
        .map_err(|e| CodecError::EncodingFailed(format!("failed to allocate encode frame: {e}")))
}

fn write_nv12_into_frame(
    nv12: &[u8],
    frame: &mut GenericDmaVideoFrame,
    display_resolution: Resolution,
    coded_resolution: Resolution,
) -> Result<(), CodecError> {
    let pitches = frame.get_plane_pitch();
    if pitches.len() < 2 {
        return Err(CodecError::EncodingFailed(
            "encode frame does not have NV12 pitches".to_string(),
        ));
    }

    let y_visible_bytes =
        (display_resolution.width as usize) * (display_resolution.height as usize);
    let uv_visible_rows = (display_resolution.height as usize) / 2;
    let uv_visible_bytes = (display_resolution.width as usize) * uv_visible_rows;

    let mapping = frame
        .map_mut()
        .map_err(|e| CodecError::EncodingFailed(format!("failed to map encode frame: {e}")))?;
    let planes = mapping.get();
    if planes.len() < 2 {
        return Err(CodecError::EncodingFailed(
            "encode frame does not have NV12 planes".to_string(),
        ));
    }

    let mut y_plane = planes[0].borrow_mut();
    let mut uv_plane = planes[1].borrow_mut();

    for row in 0..display_resolution.height as usize {
        let src_start = row * display_resolution.width as usize;
        let src_end = src_start + display_resolution.width as usize;
        let dst_start = row * pitches[0];
        let dst_end = dst_start + display_resolution.width as usize;
        y_plane[dst_start..dst_end].copy_from_slice(&nv12[src_start..src_end]);
    }
    for row in display_resolution.height as usize..coded_resolution.height as usize {
        let dst_start = row * pitches[0];
        let dst_end = dst_start + display_resolution.width as usize;
        y_plane[dst_start..dst_end].fill(0);
    }

    let uv_src = &nv12[y_visible_bytes..y_visible_bytes + uv_visible_bytes];
    for row in 0..uv_visible_rows {
        let src_start = row * display_resolution.width as usize;
        let src_end = src_start + display_resolution.width as usize;
        let dst_start = row * pitches[1];
        let dst_end = dst_start + display_resolution.width as usize;
        uv_plane[dst_start..dst_end].copy_from_slice(&uv_src[src_start..src_end]);
    }
    for row in uv_visible_rows..(coded_resolution.height as usize / 2) {
        let dst_start = row * pitches[1];
        let dst_end = dst_start + display_resolution.width as usize;
        uv_plane[dst_start..dst_end].fill(128);
    }

    Ok(())
}

fn build_h264_avcc_from_annex_b(bitstream: &[u8]) -> Option<Vec<u8>> {
    let mut sps = None;
    let mut pps = None;

    for nalu in annex_b_nalus(bitstream) {
        if nalu.is_empty() {
            continue;
        }
        match nalu[0] & 0x1f {
            7 if sps.is_none() => sps = Some(nalu.to_vec()),
            8 if pps.is_none() => pps = Some(nalu.to_vec()),
            _ => {}
        }
        if sps.is_some() && pps.is_some() {
            break;
        }
    }

    let sps = sps?;
    let pps = pps?;
    if sps.len() < 4 {
        return None;
    }

    let mut avcc = Vec::with_capacity(11 + sps.len() + pps.len());
    avcc.push(1);
    avcc.push(sps[1]);
    avcc.push(sps[2]);
    avcc.push(sps[3]);
    avcc.push(0xFC | 0x03);
    avcc.push(0xE0 | 1);
    avcc.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    avcc.extend_from_slice(&sps);
    avcc.push(1);
    avcc.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    avcc.extend_from_slice(&pps);

    Some(avcc)
}

fn annex_b_nalus(data: &[u8]) -> Vec<&[u8]> {
    let mut nalus = Vec::new();
    let mut cursor = 0;

    while let Some((start_idx, start_len)) = find_start_code(data, cursor) {
        let nalu_start = start_idx + start_len;
        let end_idx = find_start_code(data, nalu_start).map_or(data.len(), |(idx, _)| idx);
        if nalu_start < end_idx {
            nalus.push(&data[nalu_start..end_idx]);
        }
        cursor = end_idx;
    }

    nalus
}

fn find_start_code(data: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut i = from;
    while i + 3 <= data.len() {
        if i + 4 <= data.len()
            && data[i] == 0
            && data[i + 1] == 0
            && data[i + 2] == 0
            && data[i + 3] == 1
        {
            return Some((i, 4));
        }
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            return Some((i, 3));
        }
        i += 1;
    }
    None
}

const fn align_16(value: u32) -> u32 {
    (value + 15) & !15
}
