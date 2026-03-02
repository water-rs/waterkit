//! AV1 software encoding (rav1e) and decoding (rav1d).

use crate::CodecError;
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

/// CPU-side frame data for software codec output (NV12 format).
pub struct CpuFrame {
    /// NV12 data: Y plane followed by interleaved UV plane.
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub timestamp_ns: u64,
}

/// AV1 software encoder using rav1e.
pub struct Av1Encoder {
    ctx: Context<u8>,
    width: usize,
    height: usize,
}

unsafe impl Send for Av1Encoder {}
unsafe impl Sync for Av1Encoder {}

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

unsafe impl Send for Av1Decoder {}
unsafe impl Sync for Av1Decoder {}

impl fmt::Debug for Av1Decoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Av1Decoder").finish()
    }
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

    /// Decode AV1 data to NV12 frames.
    pub fn decode(&mut self, data: &[u8]) -> Result<Vec<CpuFrame>, CodecError> {
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
        let send_status =
            unsafe { rav1d_lib::dav1d_send_data(self.ctx, Some(NonNull::from(&mut input))) };
        if send_status.0 != 0 {
            unsafe { rav1d_lib::dav1d_data_unref(Some(NonNull::from(&mut input))) };
            return Err(CodecError::DecodingFailed(format!(
                "rav1d send_data failed with code {}",
                send_status.0
            )));
        }

        let mut frames = Vec::new();
        loop {
            let mut picture = Dav1dPicture::default();
            let status = unsafe {
                rav1d_lib::dav1d_get_picture(self.ctx, Some(NonNull::from(&mut picture)))
            };
            if status.0 == 0 {
                let frame = Self::picture_to_cpu_frame(&picture);
                unsafe { rav1d_lib::dav1d_picture_unref(Some(NonNull::from(&mut picture))) };
                frames.push(frame?);
                continue;
            }
            if status.0 < 0
                && std::io::Error::from_raw_os_error(-status.0).kind() == ErrorKind::WouldBlock
            {
                break;
            }
            return Err(CodecError::DecodingFailed(format!(
                "rav1d get_picture failed with code {}",
                status.0
            )));
        }

        Ok(frames)
    }

    fn picture_to_cpu_frame(picture: &Dav1dPicture) -> Result<CpuFrame, CodecError> {
        let width = usize::try_from(picture.p.w).map_err(|_| {
            CodecError::DecodingFailed(format!("rav1d returned invalid width {}", picture.p.w))
        })?;
        let height = usize::try_from(picture.p.h).map_err(|_| {
            CodecError::DecodingFailed(format!("rav1d returned invalid height {}", picture.p.h))
        })?;
        if picture.p.layout != DAV1D_PIXEL_LAYOUT_I420 {
            return Err(CodecError::DecodingFailed(format!(
                "rav1d returned unsupported pixel layout {}",
                picture.p.layout
            )));
        }
        if picture.p.bpc != 8 {
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
        let y_ptr = Self::plane_ptr(picture, 0, "Y")?;
        let u_ptr = Self::plane_ptr(picture, 1, "U")?;
        let v_ptr = Self::plane_ptr(picture, 2, "V")?;

        let y_size = width * height;
        let uv_size = width * (height / 2); // Interleaved UV
        let mut nv12 = Vec::with_capacity(y_size + uv_size);

        // Copy Y plane (remove stride padding)
        for row in 0..height {
            let src = unsafe { y_ptr.add(row * y_stride) };
            let row_bytes = unsafe { slice::from_raw_parts(src, width) };
            nv12.extend_from_slice(row_bytes);
        }

        // Interleave U and V planes
        let uv_width = width / 2;
        let uv_height = height / 2;
        for row in 0..uv_height {
            let u_row = unsafe { slice::from_raw_parts(u_ptr.add(row * uv_stride), uv_width) };
            let v_row = unsafe { slice::from_raw_parts(v_ptr.add(row * uv_stride), uv_width) };
            for col in 0..uv_width {
                nv12.push(u_row[col]);
                nv12.push(v_row[col]);
            }
        }

        Ok(CpuFrame {
            data: nv12,
            width: width as u32,
            height: height as u32,
            timestamp_ns: 0,
        })
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
