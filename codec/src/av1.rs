//! AV1 software encoding (rav1e) and decoding (dav1d).

use crate::{VideoEncoder, VideoDecoder, CodecError, Frame, PixelFormat};
use rav1e::prelude::*;
use std::fmt;
use std::sync::Arc;

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
    /// Create a new AV1 encoder.
    ///
    /// # Errors
    ///
    /// Returns `CodecError::InitializationFailed` if `rav1e` context creation fails.
    pub fn new(width: usize, height: usize) -> Result<Self, CodecError> {
        let cfg = Config::new()
            .with_encoder_config(EncoderConfig {
                width,
                height,
                bit_depth: 8,
                chroma_sampling: ChromaSampling::Cs420,
                speed_settings: SpeedSettings::from_preset(6), // Faster preset for realtime
                low_latency: true,
                ..Default::default()
            })
            .with_threads(4);

        let ctx = cfg.new_context()
            .map_err(|e| CodecError::InitializationFailed(e.to_string()))?;

        Ok(Self { ctx, width, height })
    }
    
    /// Convert RGBA to I420 (YUV420 planar).
    fn rgba_to_i420(rgba: &[u8], width: usize, height: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let y_size = width * height;
        let uv_size = (width / 2) * (height / 2);
        
        let mut y_plane = vec![0u8; y_size];
        let mut u_plane = vec![0u8; uv_size];
        let mut v_plane = vec![0u8; uv_size];
        
        for row_idx in 0..height {
            for col_idx in 0..width {
                let px_idx = (row_idx * width + col_idx) * 4;
                let r_val = i32::from(rgba[px_idx]);
                let g_val = i32::from(rgba[px_idx + 1]);
                let b_val = i32::from(rgba[px_idx + 2]);
                
                // BT.601 RGB to YUV conversion
                let y_val = ((66 * r_val + 129 * g_val + 25 * b_val + 128) >> 8) + 16;
                y_plane[row_idx * width + col_idx] = u8::try_from(y_val.clamp(0, 255)).unwrap_or(0);
                
                // Subsample U and V (every 2x2 block)
                if row_idx % 2 == 0 && col_idx % 2 == 0 {
                    let u_val = ((-38 * r_val - 74 * g_val + 112 * b_val + 128) >> 8) + 128;
                    let v_val = ((112 * r_val - 94 * g_val - 18 * b_val + 128) >> 8) + 128;
                    
                    let uv_row = row_idx / 2;
                    let uv_col = col_idx / 2;
                    let uv_idx = uv_row * (width / 2) + uv_col;
                    
                    u_plane[uv_idx] = u8::try_from(u_val.clamp(0, 255)).unwrap_or(0);
                    v_plane[uv_idx] = u8::try_from(v_val.clamp(0, 255)).unwrap_or(0);
                }
            }
        }
        
        (y_plane, u_plane, v_plane)
    }
}

impl VideoEncoder for Av1Encoder {
    fn encode(&mut self, frame: &Frame) -> Result<Vec<u8>, CodecError> {
        // Validate dimensions
        if frame.width as usize != self.width || frame.height as usize != self.height {
            return Err(CodecError::EncodingFailed(format!(
                "Frame size {}x{} doesn't match encoder {}x{}", 
                frame.width, frame.height, self.width, self.height
            )));
        }
        
        // Validate data size for RGBA format
        let expected_size = self.width * self.height * 4;
        if frame.data.len() != expected_size {
            return Err(CodecError::EncodingFailed(format!(
                "Frame data size {} doesn't match expected {} for {}x{} RGBA", 
                frame.data.len(), expected_size, self.width, self.height
            )));
        }
        
        // Create rav1e frame
        let mut f = self.ctx.new_frame();
        
        // Convert input to I420 and copy to frame planes
        let (y_plane, u_plane, v_plane) = match frame.format {
            PixelFormat::Rgba | PixelFormat::Bgra => {
                let rgba_data = if frame.format == PixelFormat::Bgra {
                    // Convert BGRA to RGBA
                    let mut rgba = frame.data.as_ref().clone();
                    for chunk in rgba.chunks_exact_mut(4) {
                        chunk.swap(0, 2);
                    }
                    rgba
                } else {
                    frame.data.as_ref().clone()
                };
                Self::rgba_to_i420(&rgba_data, self.width, self.height)
            }
            PixelFormat::I420 => {
                // Already I420 - split planes
                let y_size = self.width * self.height;
                let uv_size = (self.width / 2) * (self.height / 2);
                let data = frame.data.as_ref();
                
                (
                    data[0..y_size].to_vec(),
                    data[y_size..y_size + uv_size].to_vec(),
                    data[y_size + uv_size..].to_vec(),
                )
            }
            PixelFormat::Nv12 => {
                // NV12: Y plane followed by interleaved UV
                let y_size = self.width * self.height;
                let uv_size = (self.width / 2) * (self.height / 2);
                let data = frame.data.as_ref();
                
                let y_plane = data[0..y_size].to_vec();
                let mut u_plane = vec![0u8; uv_size];
                let mut v_plane = vec![0u8; uv_size];
                
                // De-interleave UV
                let uv_data = &data[y_size..];
                for i in 0..uv_size {
                    u_plane[i] = uv_data[i * 2];
                    v_plane[i] = uv_data[i * 2 + 1];
                }
                
                (y_plane, u_plane, v_plane)
            }
        };
        
        // Copy Y plane - limit to actual height (rav1e may have internal padding)
        for (row_idx, row) in f.planes[0].rows_iter_mut().take(self.height).enumerate() {
            let src_start = row_idx * self.width;
            let src_end = src_start + self.width;
            row[..self.width].copy_from_slice(&y_plane[src_start..src_end]);
        }
        
        // Copy U plane
        let uv_width = self.width / 2;
        let uv_height = self.height / 2;
        for (row_idx, row) in f.planes[1].rows_iter_mut().take(uv_height).enumerate() {
            let src_start = row_idx * uv_width;
            let src_end = src_start + uv_width;
            row[..uv_width].copy_from_slice(&u_plane[src_start..src_end]);
        }
        
        // Copy V plane
        for (row_idx, row) in f.planes[2].rows_iter_mut().take(uv_height).enumerate() {
            let src_start = row_idx * uv_width;
            let src_end = src_start + uv_width;
            row[..uv_width].copy_from_slice(&v_plane[src_start..src_end]);
        }
        
        // Send frame to encoder
        self.ctx.send_frame(f)
            .map_err(|e| CodecError::EncodingFailed(e.to_string()))?;
        
        // Collect all available packets
        let mut output = Vec::new();
        loop {
            match self.ctx.receive_packet() {
                Ok(pkt) => {
                    output.extend_from_slice(&pkt.data);
                }
                Err(EncoderStatus::Encoded | EncoderStatus::NeedMoreData | EncoderStatus::LimitReached) => break,
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
    /// Create a new AV1 decoder.
    ///
    /// # Errors
    ///
    /// Returns `CodecError::InitializationFailed` if `dav1d` initialization fails.
    pub fn new() -> Result<Self, CodecError> {
        let settings = dav1d::Settings::new();
        let dec = dav1d::Decoder::with_settings(&settings)
            .map_err(|e| CodecError::InitializationFailed(format!("dav1d init failed: {e:?}")))?;
        
        Ok(Self { dec })
    }
}

impl VideoDecoder for Av1Decoder {
    fn decode(&mut self, data: &[u8]) -> Result<Vec<Frame>, CodecError> {
        // Send data to decoder
        self.dec.send_data(data.to_vec(), None, None, None)
            .map_err(|e| CodecError::DecodingFailed(format!("dav1d send_data failed: {e:?}")))?;
        
        let mut frames = Vec::new();
        
        // Get all available decoded pictures
        loop {
            match self.dec.get_picture() {
                Ok(pic) => {
                    let width = pic.width();
                    let height = pic.height();
                    
                    // Extract I420 data from picture
                    let y_stride = pic.stride(dav1d::PlanarImageComponent::Y);
                    let u_stride = pic.stride(dav1d::PlanarImageComponent::U);
                    let v_stride = pic.stride(dav1d::PlanarImageComponent::V);
                    
                    let y_plane = pic.plane(dav1d::PlanarImageComponent::Y);
                    let u_plane = pic.plane(dav1d::PlanarImageComponent::U);
                    let v_plane = pic.plane(dav1d::PlanarImageComponent::V);
                    
                    // Copy to contiguous I420 buffer
                    let y_size = (width * height) as usize;
                    let uv_size = ((width / 2) * (height / 2)) as usize;
                    let mut i420_data = Vec::with_capacity(y_size + uv_size * 2);
                    
                    // Copy Y
                    for row in 0..height as usize {
                        let start = row * y_stride as usize;
                        i420_data.extend_from_slice(&y_plane[start..start + width as usize]);
                    }
                    
                    // Copy U
                    let uv_width = (width / 2) as usize;
                    let uv_height = (height / 2) as usize;
                    for row in 0..uv_height {
                        let start = row * u_stride as usize;
                        i420_data.extend_from_slice(&u_plane[start..start + uv_width]);
                    }
                    
                    // Copy V
                    for row in 0..uv_height {
                        let start = row * v_stride as usize;
                        i420_data.extend_from_slice(&v_plane[start..start + uv_width]);
                    }
                    
                    frames.push(Frame {
                        data: Arc::new(i420_data),
                        width,
                        height,
                        format: PixelFormat::I420,
                        timestamp_ns: 0, // TODO: extract from picture
                    });
                }
                Err(dav1d::Error::Again) => break, // No more pictures available
                Err(e) => return Err(CodecError::DecodingFailed(format!("dav1d get_picture failed: {e:?}"))),
            }
        }
        
        Ok(frames)
    }
}

impl Default for Av1Decoder {
    fn default() -> Self {
        Self::new().expect("Failed to create default Av1Decoder")
    }
}
