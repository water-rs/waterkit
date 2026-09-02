//! Linux VA-API hardware encoding and decoding.
//!
//! This backend uses pure VA-API via `cros-codecs`.

use crate::{CodecError, DecodePacket, DecodedPixelLayout, bitstream::NalStreamConverter};
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
use cros_codecs::{BlockingMode, DecodedFormat, Fourcc, FrameLayout, Resolution};
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

/// Internal codec type for Linux implementations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecType {
    H264,
    H265,
}

fn open_vaapi_device() -> Result<(Rc<cros_codecs::libva::Display>, Arc<GbmDevice>), CodecError> {
    for path in cros_codecs::libva::DrmDeviceIterator::default() {
        let Ok(display) = cros_codecs::libva::Display::open_drm_display(&path) else {
            continue;
        };
        let Ok(gbm_device) = GbmDevice::open(&path) else {
            continue;
        };
        return Ok((display, gbm_device));
    }
    Err(CodecError::InitializationFailed(
        "no DRM render node supports both VA-API and GBM".into(),
    ))
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
    /// Native bi-planar pixel layout.
    pub layout: DecodedPixelLayout,
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
    decoder: DynStatelessVideoDecoder<GenericDmaVideoFrame>,
    gbm_device: Arc<GbmDevice>,
    codec_type: CodecType,
    coded_resolution: Resolution,
    display_resolution: Resolution,
    input_bitstream: NalStreamConverter,
    output_format: DecodedFormat,
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
        let (display, gbm_device) = open_vaapi_device()?;

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

        let input_bitstream = NalStreamConverter::new(codec_type == CodecType::H265, config)?;
        Ok(Self {
            decoder,
            gbm_device,
            codec_type,
            coded_resolution: Resolution { width, height },
            display_resolution: Resolution { width, height },
            input_bitstream,
            output_format: DecodedFormat::NV12,
        })
    }

    /// Decode compressed video data.
    pub fn decode(&mut self, packet: DecodePacket<'_>) -> Result<Vec<LinuxFrame>, CodecError> {
        let timestamp_ns = u64::try_from(packet.presentation_time().as_nanos())
            .map_err(|_| CodecError::DecodingFailed("presentation timestamp exceeds u64".into()))?;
        let annex_b = self.prepare_annex_b_packet(packet.data())?;
        if annex_b.is_empty() {
            return Ok(Vec::new());
        }

        let mut frames = Vec::new();
        let mut offset = 0;

        while offset < annex_b.len() {
            let gbm_device = Arc::clone(&self.gbm_device);
            let display_resolution = self.display_resolution;
            let coded_resolution = self.coded_resolution;
            let output_format = self.output_format;
            let mut allocate_frame = || {
                Some(Self::allocate_decode_frame(
                    &gbm_device,
                    display_resolution,
                    coded_resolution,
                    output_format,
                ))
            };
            match self
                .decoder
                .decode(timestamp_ns, &annex_b[offset..], &mut allocate_frame)
            {
                Ok(consumed) => {
                    if consumed == 0 {
                        return Err(CodecError::DecodingFailed(
                            "VA-API decoder consumed 0 bytes".to_string(),
                        ));
                    }
                    offset += consumed;
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

    /// Flushes every delayed VA-API output frame.
    pub fn drain(&mut self) -> Result<Vec<LinuxFrame>, CodecError> {
        self.decoder.flush().map_err(|error| {
            CodecError::DecodingFailed(format!("failed to flush VA-API decoder: {error}"))
        })?;
        let mut frames = Vec::new();
        self.collect_decoder_events(&mut frames)?;
        Ok(frames)
    }

    fn prepare_annex_b_packet(&mut self, data: &[u8]) -> Result<Vec<u8>, CodecError> {
        self.input_bitstream
            .convert_sample_with_parameter_sets(data)
    }

    fn allocate_decode_frame(
        gbm_device: &Arc<GbmDevice>,
        display_resolution: Resolution,
        coded_resolution: Resolution,
        output_format: DecodedFormat,
    ) -> GenericDmaVideoFrame {
        let fourcc = match output_format {
            DecodedFormat::NV12 => Fourcc::from(b"NV12"),
            DecodedFormat::I010 => Fourcc::from(b"P010"),
            unsupported => panic!("unsupported VA-API decoded format: {unsupported:?}"),
        };
        Arc::clone(gbm_device)
            .new_frame(
                fourcc,
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
                    self.output_format = stream_info.format;
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
                    let layout = match self.output_format {
                        DecodedFormat::NV12 => DecodedPixelLayout::Nv12,
                        DecodedFormat::I010 => DecodedPixelLayout::P010,
                        unsupported => {
                            return Err(CodecError::Unsupported(format!(
                                "VA-API returned unsupported decoded format {unsupported:?}"
                            )));
                        }
                    };
                    let data = copy_biplanar_from_frame(frame.as_ref(), width, height, layout)?;

                    frames.push(LinuxFrame {
                        data,
                        width,
                        height,
                        timestamp_ns,
                        layout,
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
                let (display, gbm_device) = open_vaapi_device()?;

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

fn copy_biplanar_from_frame(
    frame: &GenericDmaVideoFrame,
    width: u32,
    height: u32,
    layout: DecodedPixelLayout,
) -> Result<Vec<u8>, CodecError> {
    let pitches = frame.get_plane_pitch();
    if pitches.len() < 2 {
        return Err(CodecError::DecodingFailed(
            "decoded frame does not have bi-planar pitches".to_string(),
        ));
    }

    let mapping = frame
        .map()
        .map_err(|e| CodecError::DecodingFailed(format!("failed to map frame: {e}")))?;
    let planes = mapping.get();
    if planes.len() < 2 {
        return Err(CodecError::DecodingFailed(
            "decoded frame does not have bi-planar planes".to_string(),
        ));
    }

    let row_bytes = layout.bytes_per_row(width);
    let y_size = row_bytes * (height as usize);
    let uv_height = (height as usize) / 2;
    let uv_size = row_bytes * uv_height;
    let mut out = Vec::with_capacity(y_size + uv_size);

    copy_plane_rows(
        planes[0],
        pitches[0],
        row_bytes,
        height as usize,
        &mut out,
        "Y",
    )?;
    copy_plane_rows(planes[1], pitches[1], row_bytes, uv_height, &mut out, "UV")?;

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
    let sps_len = u16::try_from(sps.len()).ok()?;
    let pps_len = u16::try_from(pps.len()).ok()?;

    let mut avcc = Vec::with_capacity(11 + sps.len() + pps.len());
    avcc.push(1);
    avcc.push(sps[1]);
    avcc.push(sps[2]);
    avcc.push(sps[3]);
    avcc.push(0xFC | 0x03);
    avcc.push(0xE0 | 1);
    avcc.extend_from_slice(&sps_len.to_be_bytes());
    avcc.extend_from_slice(&sps);
    avcc.push(1);
    avcc.extend_from_slice(&pps_len.to_be_bytes());
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

const fn find_start_code(data: &[u8], from: usize) -> Option<(usize, usize)> {
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
