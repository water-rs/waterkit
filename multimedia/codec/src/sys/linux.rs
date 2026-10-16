//! Linux VA-API hardware encoding and decoding.
//!
//! This backend uses pure VA-API via `cros-codecs`.

use crate::{
    CodecError, DecodePacket, DecodedPixelLayout,
    bitstream::{NalStreamConverter, build_h264_avcc_from_annex_b},
};
#[cfg(test)]
use cros_codecs::codec::h264::parser::{Level, Profile, SpsBuilder};
use cros_codecs::codec::h264::parser::{Nalu, NaluType, Parser, Sps};
use cros_codecs::codec::h264::synthesizer::Synthesizer;
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
use std::io::Cursor;
use std::rc::Rc;
use std::sync::Arc;
use waterkit_video_core::{CicpColor, VideoColorInfo};

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
    color: CicpColor,
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
    pub fn new(
        codec_type: CodecType,
        width: u32,
        height: u32,
        color: VideoColorInfo,
    ) -> Result<Self, CodecError> {
        if color.dolby_vision {
            return Err(CodecError::Unsupported(
                "VA-API H.264 encoder cannot signal Dolby Vision metadata".into(),
            ));
        }
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
            color: color.cicp(),
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
            let signaled_bitstream = signal_h264_colour(&coded.bitstream, self.color)?;
            if self.codec_config.is_none() {
                self.codec_config = build_h264_avcc_from_annex_b(&signaled_bitstream);
            }
            packet.extend_from_slice(&signaled_bitstream);
        }

        Ok(packet)
    }

    /// Get codec configuration data if available.
    #[must_use]
    pub fn get_codec_config(&self) -> Option<Vec<u8>> {
        self.codec_config.clone()
    }
}

fn signal_h264_colour(annex_b: &[u8], colour: CicpColor) -> Result<Vec<u8>, CodecError> {
    let mut cursor = Cursor::new(annex_b);
    let mut output = Vec::with_capacity(annex_b.len());
    let mut found_nalu = false;
    loop {
        let nalu = match Nalu::next(&mut cursor) {
            Ok(nalu) => nalu,
            Err(error) if error == "No NAL found" && (found_nalu || annex_b.is_empty()) => break,
            Err(error) => {
                return Err(CodecError::EncodingFailed(format!(
                    "failed to parse VA-API H.264 Annex-B output: {error}"
                )));
            }
        };
        found_nalu = true;
        if nalu.header.type_ != NaluType::Sps {
            output.extend_from_slice(nalu.data.as_ref());
            continue;
        }

        let mut parser = Parser::default();
        let parsed_sps = parser.parse_sps(&nalu).map_err(|error| {
            CodecError::EncodingFailed(format!("failed to parse VA-API H.264 SPS: {error}"))
        })?;
        let mut sps = clone_sps(parsed_sps.as_ref());
        sps.vui_parameters_present_flag = true;
        sps.vui_parameters.video_signal_type_present_flag = true;
        sps.vui_parameters.video_format = 5;
        sps.vui_parameters.video_full_range_flag = colour.full_range;
        sps.vui_parameters.colour_description_present_flag = true;
        sps.vui_parameters.colour_primaries = colour.primaries;
        sps.vui_parameters.transfer_characteristics = colour.transfer;
        sps.vui_parameters.matrix_coefficients = colour.matrix;
        Synthesizer::<Sps, _>::synthesize(nalu.header.ref_idc, &sps, &mut output, true).map_err(
            |error| {
                CodecError::EncodingFailed(format!(
                    "failed to synthesize color-tagged H.264 SPS: {error}"
                ))
            },
        )?;
    }
    Ok(output)
}

fn clone_sps(source: &Sps) -> Sps {
    Sps {
        seq_parameter_set_id: source.seq_parameter_set_id,
        profile_idc: source.profile_idc,
        constraint_set0_flag: source.constraint_set0_flag,
        constraint_set1_flag: source.constraint_set1_flag,
        constraint_set2_flag: source.constraint_set2_flag,
        constraint_set3_flag: source.constraint_set3_flag,
        constraint_set4_flag: source.constraint_set4_flag,
        constraint_set5_flag: source.constraint_set5_flag,
        level_idc: source.level_idc,
        chroma_format_idc: source.chroma_format_idc,
        separate_colour_plane_flag: source.separate_colour_plane_flag,
        bit_depth_luma_minus8: source.bit_depth_luma_minus8,
        bit_depth_chroma_minus8: source.bit_depth_chroma_minus8,
        qpprime_y_zero_transform_bypass_flag: source.qpprime_y_zero_transform_bypass_flag,
        seq_scaling_matrix_present_flag: source.seq_scaling_matrix_present_flag,
        scaling_lists_4x4: source.scaling_lists_4x4,
        scaling_lists_8x8: source.scaling_lists_8x8,
        log2_max_frame_num_minus4: source.log2_max_frame_num_minus4,
        pic_order_cnt_type: source.pic_order_cnt_type,
        log2_max_pic_order_cnt_lsb_minus4: source.log2_max_pic_order_cnt_lsb_minus4,
        delta_pic_order_always_zero_flag: source.delta_pic_order_always_zero_flag,
        offset_for_non_ref_pic: source.offset_for_non_ref_pic,
        offset_for_top_to_bottom_field: source.offset_for_top_to_bottom_field,
        num_ref_frames_in_pic_order_cnt_cycle: source.num_ref_frames_in_pic_order_cnt_cycle,
        offset_for_ref_frame: source.offset_for_ref_frame,
        max_num_ref_frames: source.max_num_ref_frames,
        gaps_in_frame_num_value_allowed_flag: source.gaps_in_frame_num_value_allowed_flag,
        pic_width_in_mbs_minus1: source.pic_width_in_mbs_minus1,
        pic_height_in_map_units_minus1: source.pic_height_in_map_units_minus1,
        frame_mbs_only_flag: source.frame_mbs_only_flag,
        mb_adaptive_frame_field_flag: source.mb_adaptive_frame_field_flag,
        direct_8x8_inference_flag: source.direct_8x8_inference_flag,
        frame_cropping_flag: source.frame_cropping_flag,
        frame_crop_left_offset: source.frame_crop_left_offset,
        frame_crop_right_offset: source.frame_crop_right_offset,
        frame_crop_top_offset: source.frame_crop_top_offset,
        frame_crop_bottom_offset: source.frame_crop_bottom_offset,
        expected_delta_per_pic_order_cnt_cycle: source.expected_delta_per_pic_order_cnt_cycle,
        vui_parameters_present_flag: source.vui_parameters_present_flag,
        vui_parameters: source.vui_parameters.clone(),
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

const fn align_16(value: u32) -> u32 {
    (value + 15) & !15
}

/// Open a hardware decoder for a crate-level codec type.
pub fn open_decoder(
    codec: crate::CodecType,
    config: Option<&[u8]>,
    width: u32,
    height: u32,
) -> Result<crate::DecoderInner, CodecError> {
    let codec = match codec {
        crate::CodecType::H264 => CodecType::H264,
        crate::CodecType::H265 => CodecType::H265,
        crate::CodecType::Av1 => unreachable!(),
    };
    LinuxDecoder::new(codec, config, width, height).map(crate::DecoderInner::Linux)
}

/// Open a hardware encoder for a crate-level codec type.
pub fn open_encoder(
    codec: crate::CodecType,
    width: u32,
    height: u32,
    color: VideoColorInfo,
) -> Result<crate::EncoderInner, CodecError> {
    let codec = match codec {
        crate::CodecType::H264 => CodecType::H264,
        crate::CodecType::H265 => CodecType::H265,
        crate::CodecType::Av1 => unreachable!(),
    };
    LinuxEncoder::new(codec, width, height, color).map(crate::EncoderInner::Linux)
}

#[cfg(test)]
mod color_tests {
    use super::*;

    #[test]
    fn signal_h264_colour_rewrites_sps_and_preserves_other_nalus() {
        let sps = SpsBuilder::new()
            .profile_idc(Profile::High)
            .level_idc(Level::L3)
            .resolution(16, 16)
            .frame_mbs_only_flag(true)
            .build();
        let mut annex_b = Vec::new();
        Synthesizer::<Sps, _>::synthesize(3, &sps, &mut annex_b, true)
            .expect("synthesize test SPS");
        let other_nalu = [0x00, 0x00, 0x00, 0x01, 0x09, 0xf0];
        annex_b.extend_from_slice(&other_nalu);
        let colour = CicpColor {
            primaries: 6,
            transfer: 1,
            matrix: 6,
            full_range: true,
        };

        let rewritten = signal_h264_colour(&annex_b, colour).expect("rewrite SPS color");
        let mut cursor = Cursor::new(rewritten.as_slice());
        let mut parser = Parser::default();
        let mut found_sps = false;
        let mut found_other = false;
        while let Ok(nalu) = Nalu::next(&mut cursor) {
            if nalu.header.type_ == NaluType::Sps {
                let sps = parser.parse_sps(&nalu).expect("parse rewritten SPS");
                let vui = &sps.vui_parameters;
                assert!(sps.vui_parameters_present_flag);
                assert!(vui.video_signal_type_present_flag);
                assert_eq!(vui.video_format, 5);
                assert!(vui.video_full_range_flag);
                assert!(vui.colour_description_present_flag);
                assert_eq!(vui.colour_primaries, colour.primaries);
                assert_eq!(vui.transfer_characteristics, colour.transfer);
                assert_eq!(vui.matrix_coefficients, colour.matrix);
                found_sps = true;
            } else {
                assert_eq!(nalu.data.as_ref(), other_nalu);
                found_other = true;
            }
        }
        assert!(found_sps);
        assert!(found_other);
    }
}
