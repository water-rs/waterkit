//! AV1 software encoding (rav1e) and decoding (rav1d).

use crate::{CodecError, DecodePacket, DecodedPixelLayout};
use rav1d::include::dav1d::data::Dav1dData;
use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
use rav1d::include::dav1d::headers::DAV1D_PIXEL_LAYOUT_I420;
use rav1d::include::dav1d::picture::Dav1dPicture;
use rav1d::src::lib as rav1d_lib;
use rav1e::prelude::*;
use std::fmt;
use std::io::ErrorKind;
use std::mem::MaybeUninit;
use std::ptr::NonNull;
use std::{ptr, slice};

/// CPU-side frame data for software codec output (NV12 or P010 format).
pub struct CpuFrame {
    /// Bi-planar data: Y plane followed by interleaved UV plane.
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub timestamp_ns: u64,
    pub layout: DecodedPixelLayout,
    /// CICP color description carried by the decoded AV1 sequence header.
    #[cfg(not(any(target_vendor = "apple", target_os = "android", target_arch = "wasm32")))]
    pub color: Av1ColorDescription,
}

/// Coding-independent color metadata attached to an AV1 frame.
#[cfg(not(any(target_vendor = "apple", target_os = "android", target_arch = "wasm32")))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Av1ColorDescription {
    /// H.273 color-primaries code point.
    pub primaries: u8,
    /// H.273 transfer-characteristics code point.
    pub transfer: u8,
    /// H.273 matrix-coefficients code point.
    pub matrix: u8,
    /// Whether YUV samples use full rather than studio range.
    pub full_range: bool,
}

/// AV1 software encoder using rav1e.
pub struct Av1Encoder {
    ctx: Context<u8>,
    width: usize,
    height: usize,
}

impl fmt::Debug for Av1Encoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Av1Encoder")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl Av1Encoder {
    pub fn new(width: usize, height: usize) -> Result<Self, CodecError> {
        let cfg = Config::new()
            .with_encoder_config(EncoderConfig {
                width,
                height,
                bit_depth: 8,
                chroma_sampling: ChromaSampling::Cs420,
                speed_settings: SpeedSettings::from_preset(6),
                low_latency: true,
                ..Default::default()
            })
            .with_threads(4);

        let ctx = cfg
            .new_context()
            .map_err(|e| CodecError::InitializationFailed(e.to_string()))?;

        Ok(Self { ctx, width, height })
    }

    /// Encode NV12 data to AV1.
    pub fn encode_nv12(&mut self, nv12: &[u8]) -> Result<Vec<u8>, CodecError> {
        let y_size = self.width * self.height;
        let uv_size = y_size / 2; // Interleaved UV
        let expected_size = y_size + uv_size;

        if nv12.len() != expected_size {
            return Err(CodecError::EncodingFailed(format!(
                "Data size {} doesn't match expected {} for {}x{} NV12",
                nv12.len(),
                expected_size,
                self.width,
                self.height
            )));
        }

        let mut f = self.ctx.new_frame();

        // Copy Y plane
        let y_data = &nv12[..y_size];
        for (row_idx, row) in f.planes[0].rows_iter_mut().take(self.height).enumerate() {
            let src_start = row_idx * self.width;
            let src_end = src_start + self.width;
            row[..self.width].copy_from_slice(&y_data[src_start..src_end]);
        }

        // De-interleave UV and copy to U/V planes
        let uv_data = &nv12[y_size..];
        let uv_width = self.width / 2;
        let uv_height = self.height / 2;

        // First copy U plane
        for (row_idx, u_row) in f.planes[1].rows_iter_mut().take(uv_height).enumerate() {
            for (col_idx, pixel) in u_row.iter_mut().enumerate().take(uv_width) {
                let src_idx = row_idx * self.width + col_idx * 2;
                *pixel = uv_data[src_idx];
            }
        }

        // Then copy V plane
        for (row_idx, v_row) in f.planes[2].rows_iter_mut().take(uv_height).enumerate() {
            for (col_idx, pixel) in v_row.iter_mut().enumerate().take(uv_width) {
                let src_idx = row_idx * self.width + col_idx * 2 + 1;
                *pixel = uv_data[src_idx];
            }
        }

        self.ctx
            .send_frame(f)
            .map_err(|e| CodecError::EncodingFailed(e.to_string()))?;

        let mut output = Vec::new();
        loop {
            match self.ctx.receive_packet() {
                Ok(pkt) => output.extend_from_slice(&pkt.data),
                Err(
                    EncoderStatus::Encoded
                    | EncoderStatus::NeedMoreData
                    | EncoderStatus::LimitReached,
                ) => break,
                Err(e) => return Err(CodecError::EncodingFailed(e.to_string())),
            }
        }

        Ok(output)
    }
}

/// AV1 software decoder using rav1d.
pub struct Av1Decoder {
    ctx: Option<Dav1dContext>,
}

impl fmt::Debug for Av1Decoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Av1Decoder").finish()
    }
}

struct PictureLayout {
    width: usize,
    height: usize,
    width_u32: u32,
    height_u32: u32,
    bit_depth: usize,
    y_stride: usize,
    uv_stride: usize,
    y_min_stride: usize,
    uv_min_stride: usize,
    uv_width: usize,
    uv_height: usize,
    y_size: usize,
    uv_size: usize,
}

impl Av1Decoder {
    pub fn new() -> Result<Self, CodecError> {
        let mut settings = MaybeUninit::<Dav1dSettings>::uninit();
        unsafe {
            rav1d_lib::dav1d_default_settings(NonNull::from(&mut settings).cast());
        }
        let mut settings = unsafe { settings.assume_init() };
        let mut ctx = None;
        let status = unsafe {
            rav1d_lib::dav1d_open(
                Some(NonNull::from(&mut ctx)),
                Some(NonNull::from(&mut settings)),
            )
        };
        if status.0 != 0 {
            return Err(CodecError::InitializationFailed(format!(
                "rav1d open failed with code {}",
                status.0
            )));
        }
        Ok(Self { ctx })
    }

    /// Decode AV1 data to NV12 or P010 frames without discarding source precision.
    pub fn decode(&mut self, packet: DecodePacket<'_>) -> Result<Vec<CpuFrame>, CodecError> {
        let data = packet.data();
        let mut input = Dav1dData::default();
        let input_ptr =
            unsafe { rav1d_lib::dav1d_data_create(Some(NonNull::from(&mut input)), data.len()) };
        if input_ptr.is_null() {
            return Err(CodecError::DecodingFailed(
                "rav1d data_create returned null".to_string(),
            ));
        }
        unsafe {
            ptr::copy_nonoverlapping(data.as_ptr(), input_ptr, data.len());
        }
        input.m.timestamp = i64::try_from(packet.presentation_time().as_nanos())
            .map_err(|_| CodecError::DecodingFailed("presentation timestamp exceeds i64".into()))?;
        let send_status =
            unsafe { rav1d_lib::dav1d_send_data(self.ctx, Some(NonNull::from(&mut input))) };
        if send_status.0 != 0 {
            unsafe { rav1d_lib::dav1d_data_unref(Some(NonNull::from(&mut input))) };
            return Err(CodecError::DecodingFailed(format!(
                "rav1d send_data failed with code {}",
                send_status.0
            )));
        }

        self.collect_pictures()
    }

    /// Returns every delayed rav1d output frame.
    pub fn drain(&mut self) -> Result<Vec<CpuFrame>, CodecError> {
        self.collect_pictures()
    }

    fn collect_pictures(&mut self) -> Result<Vec<CpuFrame>, CodecError> {
        let mut frames = Vec::new();
        let mut saw_would_block_once = false;
        loop {
            let mut picture = Dav1dPicture::default();
            let status = unsafe {
                rav1d_lib::dav1d_get_picture(self.ctx, Some(NonNull::from(&mut picture)))
            };
            if status.0 == 0 {
                let frame = Self::picture_to_cpu_frame(&picture);
                unsafe { rav1d_lib::dav1d_picture_unref(Some(NonNull::from(&mut picture))) };
                frames.push(frame?);
                saw_would_block_once = false;
                continue;
            }
            if status.0 < 0
                && std::io::Error::from_raw_os_error(-status.0).kind() == ErrorKind::WouldBlock
            {
                if saw_would_block_once {
                    break;
                }
                // `dav1d` may require one extra poll after the first EAGAIN to drain
                // frames that became available during internal scheduling.
                saw_would_block_once = true;
                continue;
            }
            return Err(CodecError::DecodingFailed(format!(
                "rav1d get_picture failed with code {}",
                status.0
            )));
        }

        Ok(frames)
    }

    fn picture_to_cpu_frame(picture: &Dav1dPicture) -> Result<CpuFrame, CodecError> {
        let layout = Self::picture_layout(picture)?;
        let y_ptr = Self::plane_ptr(picture, 0, "Y")?;
        let u_ptr = Self::plane_ptr(picture, 1, "U")?;
        let v_ptr = Self::plane_ptr(picture, 2, "V")?;

        let pixel_layout = match layout.bit_depth {
            8 => DecodedPixelLayout::Nv12,
            10 => DecodedPixelLayout::P010,
            bit_depth => {
                return Err(CodecError::Unsupported(format!(
                    "AV1 {bit_depth}-bit output has no supported bi-planar GPU layout"
                )));
            }
        };
        let mut biplanar = Vec::with_capacity(layout.y_size + layout.uv_size);
        #[cfg(not(any(target_vendor = "apple", target_os = "android", target_arch = "wasm32")))]
        let sequence_header = unsafe {
            picture
                .seq_hdr
                .ok_or_else(|| {
                    CodecError::DecodingFailed("rav1d returned no AV1 sequence header".into())
                })?
                .as_ref()
        };
        #[cfg(not(any(target_vendor = "apple", target_os = "android", target_arch = "wasm32")))]
        let color = Av1ColorDescription {
            primaries: u8::try_from(sequence_header.pri).map_err(|_| {
                CodecError::DecodingFailed("AV1 color primaries exceed CICP range".into())
            })?,
            transfer: u8::try_from(sequence_header.trc).map_err(|_| {
                CodecError::DecodingFailed("AV1 transfer characteristics exceed CICP range".into())
            })?,
            matrix: u8::try_from(sequence_header.mtrx).map_err(|_| {
                CodecError::DecodingFailed("AV1 matrix coefficients exceed CICP range".into())
            })?,
            full_range: sequence_header.color_range != 0,
        };

        if layout.bit_depth == 8 {
            Self::copy_8_bit_i420_to_nv12(&layout, y_ptr, u_ptr, v_ptr, &mut biplanar);
        } else {
            Self::copy_10_bit_i420_to_p010(&layout, y_ptr, u_ptr, v_ptr, &mut biplanar);
        }

        Ok(CpuFrame {
            data: biplanar,
            width: layout.width_u32,
            height: layout.height_u32,
            timestamp_ns: u64::try_from(picture.m.timestamp).map_err(|_| {
                CodecError::DecodingFailed(format!(
                    "rav1d returned invalid timestamp {}",
                    picture.m.timestamp
                ))
            })?,
            layout: pixel_layout,
            #[cfg(not(any(
                target_vendor = "apple",
                target_os = "android",
                target_arch = "wasm32"
            )))]
            color,
        })
    }

    fn picture_layout(picture: &Dav1dPicture) -> Result<PictureLayout, CodecError> {
        let width = usize::try_from(picture.p.w).map_err(|_| {
            CodecError::DecodingFailed(format!("rav1d returned invalid width {}", picture.p.w))
        })?;
        let width_u32 = u32::try_from(width).map_err(|_| {
            CodecError::DecodingFailed(format!("rav1d width {width} exceeds supported range"))
        })?;
        let height = usize::try_from(picture.p.h).map_err(|_| {
            CodecError::DecodingFailed(format!("rav1d returned invalid height {}", picture.p.h))
        })?;
        let height_u32 = u32::try_from(height).map_err(|_| {
            CodecError::DecodingFailed(format!("rav1d height {height} exceeds supported range"))
        })?;
        if picture.p.layout != DAV1D_PIXEL_LAYOUT_I420 {
            return Err(CodecError::DecodingFailed(format!(
                "rav1d returned unsupported pixel layout {}",
                picture.p.layout
            )));
        }
        let bit_depth = usize::try_from(picture.p.bpc).map_err(|_| {
            CodecError::DecodingFailed(format!(
                "rav1d returned invalid bit depth {}",
                picture.p.bpc
            ))
        })?;
        if !matches!(bit_depth, 8 | 10 | 12) {
            return Err(CodecError::DecodingFailed(format!(
                "rav1d returned unsupported bit depth {}",
                picture.p.bpc
            )));
        }
        let y_stride = usize::try_from(picture.stride[0]).map_err(|_| {
            CodecError::DecodingFailed(format!(
                "rav1d returned invalid Y stride {}",
                picture.stride[0]
            ))
        })?;
        let uv_stride = usize::try_from(picture.stride[1]).map_err(|_| {
            CodecError::DecodingFailed(format!(
                "rav1d returned invalid UV stride {}",
                picture.stride[1]
            ))
        })?;

        let sample_bytes = if bit_depth <= 8 { 1 } else { 2 };
        let uv_width = width.div_ceil(2);
        let uv_height = height.div_ceil(2);
        let y_size = width * height * sample_bytes;
        let uv_size = uv_width * uv_height * 2 * sample_bytes;
        let y_min_stride = width
            .checked_mul(sample_bytes)
            .ok_or_else(|| CodecError::DecodingFailed("rav1d Y stride overflow".to_string()))?;
        if y_stride < y_min_stride {
            return Err(CodecError::DecodingFailed(format!(
                "rav1d Y stride {y_stride} is smaller than required {y_min_stride}"
            )));
        }
        let uv_min_stride = uv_width
            .checked_mul(sample_bytes)
            .ok_or_else(|| CodecError::DecodingFailed("rav1d UV stride overflow".to_string()))?;
        if uv_stride < uv_min_stride {
            return Err(CodecError::DecodingFailed(format!(
                "rav1d UV stride {uv_stride} is smaller than required {uv_min_stride}"
            )));
        }

        Ok(PictureLayout {
            width,
            height,
            width_u32,
            height_u32,
            bit_depth,
            y_stride,
            uv_stride,
            y_min_stride,
            uv_min_stride,
            uv_width,
            uv_height,
            y_size,
            uv_size,
        })
    }

    fn copy_8_bit_i420_to_nv12(
        layout: &PictureLayout,
        y_ptr: *const u8,
        u_ptr: *const u8,
        v_ptr: *const u8,
        nv12: &mut Vec<u8>,
    ) {
        for row in 0..layout.height {
            let src = unsafe { y_ptr.add(row * layout.y_stride) };
            let row_bytes = unsafe { slice::from_raw_parts(src, layout.width) };
            nv12.extend_from_slice(row_bytes);
        }
        for row in 0..layout.uv_height {
            let u_row = unsafe {
                slice::from_raw_parts(u_ptr.add(row * layout.uv_stride), layout.uv_width)
            };
            let v_row = unsafe {
                slice::from_raw_parts(v_ptr.add(row * layout.uv_stride), layout.uv_width)
            };
            for col in 0..layout.uv_width {
                nv12.push(u_row[col]);
                nv12.push(v_row[col]);
            }
        }
    }

    fn copy_10_bit_i420_to_p010(
        layout: &PictureLayout,
        y_ptr: *const u8,
        u_ptr: *const u8,
        v_ptr: *const u8,
        p010: &mut Vec<u8>,
    ) {
        for row in 0..layout.height {
            let src = unsafe { y_ptr.add(row * layout.y_stride) };
            let row_bytes = unsafe { slice::from_raw_parts(src, layout.y_min_stride) };
            for col in 0..layout.width {
                let i = col * 2;
                let value = u16::from_ne_bytes([row_bytes[i], row_bytes[i + 1]]);
                p010.extend_from_slice(&(value << 6).to_le_bytes());
            }
        }
        for row in 0..layout.uv_height {
            let u_row = unsafe {
                slice::from_raw_parts(u_ptr.add(row * layout.uv_stride), layout.uv_min_stride)
            };
            let v_row = unsafe {
                slice::from_raw_parts(v_ptr.add(row * layout.uv_stride), layout.uv_min_stride)
            };
            for col in 0..layout.uv_width {
                let i = col * 2;
                let u = u16::from_ne_bytes([u_row[i], u_row[i + 1]]);
                let v = u16::from_ne_bytes([v_row[i], v_row[i + 1]]);
                p010.extend_from_slice(&(u << 6).to_le_bytes());
                p010.extend_from_slice(&(v << 6).to_le_bytes());
            }
        }
    }

    fn plane_ptr(
        picture: &Dav1dPicture,
        index: usize,
        name: &'static str,
    ) -> Result<*const u8, CodecError> {
        let plane = picture.data[index].ok_or_else(|| {
            CodecError::DecodingFailed(format!("rav1d returned missing {name} plane"))
        })?;
        Ok(plane.cast::<u8>().as_ptr().cast_const())
    }
}

impl Drop for Av1Decoder {
    fn drop(&mut self) {
        unsafe { rav1d_lib::dav1d_close(Some(NonNull::from(&mut self.ctx))) };
    }
}
