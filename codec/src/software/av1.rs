//! AV1 software encoding (rav1e) and decoding (dav1d).

use crate::CodecError;
use rav1e::prelude::*;
use std::fmt;

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

/// AV1 software decoder using dav1d.
pub struct Av1Decoder {
    dec: dav1d::Decoder,
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
        let settings = dav1d::Settings::new();
        let dec = dav1d::Decoder::with_settings(&settings)
            .map_err(|e| CodecError::InitializationFailed(format!("dav1d init failed: {e:?}")))?;

        Ok(Self { dec })
    }

    /// Decode AV1 data to NV12 frames.
    pub fn decode(&mut self, data: &[u8]) -> Result<Vec<CpuFrame>, CodecError> {
        self.dec
            .send_data(data.to_vec(), None, None, None)
            .map_err(|e| CodecError::DecodingFailed(format!("dav1d send_data failed: {e:?}")))?;

        let mut frames = Vec::new();

        loop {
            match self.dec.get_picture() {
                Ok(pic) => {
                    let width = pic.width();
                    let height = pic.height();

                    // Convert I420 to NV12 (just interleave UV)
                    let nv12 = Self::i420_to_nv12(&pic);

                    frames.push(CpuFrame {
                        data: nv12,
                        width,
                        height,
                        timestamp_ns: 0,
                    });
                }
                Err(dav1d::Error::Again) => break,
                Err(e) => {
                    return Err(CodecError::DecodingFailed(format!(
                        "dav1d get_picture failed: {e:?}"
                    )));
                }
            }
        }

        Ok(frames)
    }

    /// Convert I420 to NV12 (interleave UV planes).
    fn i420_to_nv12(pic: &dav1d::Picture) -> Vec<u8> {
        let width = pic.width() as usize;
        let height = pic.height() as usize;

        let y_stride = pic.stride(dav1d::PlanarImageComponent::Y) as usize;
        let u_stride = pic.stride(dav1d::PlanarImageComponent::U) as usize;
        let v_stride = pic.stride(dav1d::PlanarImageComponent::V) as usize;

        let y_plane = pic.plane(dav1d::PlanarImageComponent::Y);
        let u_plane = pic.plane(dav1d::PlanarImageComponent::U);
        let v_plane = pic.plane(dav1d::PlanarImageComponent::V);

        let y_size = width * height;
        let uv_size = width * (height / 2); // Interleaved UV
        let mut nv12 = Vec::with_capacity(y_size + uv_size);

        // Copy Y plane (remove stride padding)
        for row in 0..height {
            nv12.extend_from_slice(&y_plane[row * y_stride..row * y_stride + width]);
        }

        // Interleave U and V planes
        let uv_width = width / 2;
        let uv_height = height / 2;
        for row in 0..uv_height {
            for col in 0..uv_width {
                nv12.push(u_plane[row * u_stride + col]);
                nv12.push(v_plane[row * v_stride + col]);
            }
        }

        nv12
    }
}
