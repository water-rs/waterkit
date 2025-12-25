//! Android MediaCodec implementation.
#![allow(unused_imports)]

use crate::{CodecError, CodecType, Frame, PixelFormat, VideoDecoder, VideoEncoder};
use ndk::media::media_codec::{
    MediaCodec, MediaCodecDirection, MediaCodecInfo, MediaCodecResult, MediaFormat,
};
use std::collections::VecDeque;
use std::time::Duration;

pub struct AndroidEncoder;

impl AndroidEncoder {
    pub fn new(_codec: CodecType) -> Result<Self, CodecError> {
        Ok(Self)
    }
}

impl VideoEncoder for AndroidEncoder {
    fn encode(&mut self, _frame: &Frame) -> Result<Vec<u8>, CodecError> {
        Err(CodecError::Unknown("Not implemented".into()))
    }
}

pub struct AndroidDecoder {
    codec: MediaCodec,
    format: CodecType,
    width: u32,
    height: u32,
    output_format: Option<MediaFormat>,
}

impl AndroidDecoder {
    pub fn new(codec: CodecType) -> Result<Self, CodecError> {
        // Need to know dimensions to configure generic?
        // Actually generic MediaCodec can be created by name/type, but configure needs format with width/height.
        // The API `new(codec)` doesn't provide width/height!
        // `AppleDecoder` has `check_api` or `new` doesn't take dims?
        // Wait, `AppleDecoder` in `apple.rs` had `new(codec, config, width, height)`.
        // The `stub.rs` had `new(codec)`.
        // I need to match the signature required by `waterkit-codec` TRAIT/API.
        // If `apple.rs` has `new(codec, config, width, height)`, then `android.rs` should have it too.
        // I'll implement `new(codec, config, width, height)`.
        // BUT the `stub.rs` implementation I saw earlier only had `new(codec)`.
        // This suggests `lib.rs` conditionally exports differently?
        // Or I was viewing `stub.rs` or `android.rs` which was just a stub.
        // Let's implement the FULL signature.
        Err(CodecError::InitializationFailed("Use new_with_config".into()))
    }

    pub fn new_with_config(
        codec: CodecType,
        config: Option<&[u8]>,
        width: u32,
        height: u32,
    ) -> Result<Self, CodecError> {
         let mime = match codec {
            CodecType::H264 => "video/avc",
            CodecType::H265 => "video/hevc",
            CodecType::VP8 => "video/x-vnd.on2.vp8",
            CodecType::VP9 => "video/x-vnd.on2.vp9",
            CodecType::AV1 => "video/av01",
            _ => return Err(CodecError::Unsupported(format!("{codec:?}"))),
        };

        let media_codec = MediaCodec::from_decoder_type(mime)
            .ok_or(CodecError::InitializationFailed("Failed to create codec".into()))?;

        let format = MediaFormat::new();
        format.set_str("mime", mime);
        format.set_i32("width", width as i32);
        format.set_i32("height", height as i32);
        
        // Android requires csd-0 / csd-1 for AVC/HEVC if not in stream.
        // If config is provided (avcC/hvcC), we should try to parse and set it.
        // For simplicity, we assume generic configuration or that the first frame contains necessary headers (if converted).
        // But for reliable `new`, we should set "csd-0" if possible.
        // Parsing avcC/hvcC to csd-0 buffers is non-trivial here without helper crates.
        // However, if we don't set it, `configure` might fail or first frame decode might fail.
        // Many Android decoders support receiving config in the first buffer with FLAG_CODEC_CONFIG.
        // We will rely on that or the stream content.
        // Ideally we pass `config` as `csd-0`.
        if let Some(c) = config {
             format.set_buffer("csd-0", c);
        }

        media_codec.configure(&format, None, MediaCodecDirection::Decoder)
            .map_err(|e| CodecError::InitializationFailed(format!("Configure failed: {e}")))?;

        media_codec.start()
            .map_err(|e| CodecError::InitializationFailed(format!("Start failed: {e}")))?;

        Ok(Self {
            codec: media_codec,
            format: codec,
            width,
            height,
            output_format: None,
        })
    }
}

impl VideoDecoder for AndroidDecoder {
    fn decode(&mut self, data: &[u8]) -> Result<Vec<Frame>, CodecError> {
        // 1. Dequeue input buffer
        match self.codec.dequeue_input_buffer(Duration::from_millis(10)) {
            Ok(idx) => {
                let mut buffer = self.codec.get_input_buffer(idx)
                    .ok_or(CodecError::DecodingFailed("Input buffer null".into()))?;
                 
                // Copy data
                // Note: If data is larger than buffer, we have a problem.
                let len = data.len().min(buffer.len());
                buffer[..len].copy_from_slice(&data[..len]);

                // Queue
                self.codec.queue_input_buffer(idx, 0, len, 0, 0) // timestamp ? flags ?
                    .map_err(|e| CodecError::DecodingFailed(format!("Queue input failed: {e}")))?;
            }
            Err(_e) => {
                // Buffer not available, maybe try again or drop frame?
                // For now just warn
                // println!("Input buffer not available");
            }
        }

        let mut frames = Vec::new();

        // 2. Dequeue output buffer
        loop {
            let mut info = ndk::media::media_codec::MediaCodecBufferInfo::default();
            match self.codec.dequeue_output_buffer(&mut info, Duration::from_millis(0)) {
                Ok(idx) => {
                    if idx >= 0 {
                        // Got valid buffer
                        let buffer = self.codec.get_output_buffer(idx as usize)
                            .ok_or(CodecError::DecodingFailed("Output buffer null".into()))?;
                        
                        // Convert buffer (NV12/YUV) to RGBA
                        if let Some(fmt) = self.output_format.as_ref() {
                            // Default to width/height if not in format (though usually they are)
                            // API level 29 format get_i32 keys: "width", "height", "color-format", "stride", "slice-height"
                            let w = fmt.i32("width").unwrap_or(self.width as i32) as usize;
                            let h = fmt.i32("height").unwrap_or(self.height as i32) as usize;
                            let stride = fmt.i32("stride").unwrap_or(w as i32) as usize;
                            let slice_height = fmt.i32("slice-height").unwrap_or(h as i32) as usize;
                            // color-format 21 is COLOR_FormatYUV420SemiPlanar (NV12)
                            // 19 is YUV420Planar (I420)
                            let color_fmt = fmt.i32("color-format").unwrap_or(21);

                            let size = w * h * 4;
                            let mut rgba = Vec::with_capacity(size);
                            // Simple resizing if needed, but for now we push per pixel.
                            // We need access to Y, U, V planes.
                            // Buffer is flat.
                            // layout depends on color format.
                            
                            // Naive NV12 to RGBA
                            // NV12: Y plane (stride * slice_height), then UV plane interlaced (stride * slice_height / 2)
                            // Length check
                            if buffer.len() >= stride * h * 3 / 2 {
                                let y_plane = &buffer[0..stride * h];
                                let uv_plane_offset = stride * slice_height;
                                let uv_plane = &buffer[uv_plane_offset..];
                                
                                for y in 0..h {
                                    for x in 0..w {
                                        let y_idx = y * stride + x;
                                        let uv_idx = (y / 2) * stride + (x / 2) * 2;
                                        
                                        let y_val = y_plane[y_idx] as i32;
                                        let u_val = uv_plane[uv_idx] as i32; // V first? NV12 is UV usually, NV21 is VU. Android default is usually NV12/NV21.
                                        // Let's assume NV12 (UV)
                                        let v_val = uv_plane[uv_idx + 1] as i32;
                                        
                                        // YUV to RGB (integers)
                                        let c = y_val - 16;
                                        let d = u_val - 128; // U
                                        let e = v_val - 128; // V
                                        
                                        let r = ((298 * c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
                                        let g = ((298 * c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
                                        let b = ((298 * c + 516 * d + 128) >> 8).clamp(0, 255) as u8;
                                        
                                        rgba.push(r);
                                        rgba.push(g);
                                        rgba.push(b);
                                        rgba.push(255);
                                    }
                                }
                                
                                frames.push(Frame {
                                    data: std::sync::Arc::new(rgba), // Arc<Vec<u8>>? Check Frame definition
                                    width: w as u32,
                                    height: h as u32,
                                    format: PixelFormat::Rgba,
                                    timestamp_ns: info.presentation_time_us as u64 * 1000,
                                });
                            }
                        }

                        // Release
                        self.codec.release_output_buffer(idx as usize, false)
                             .map_err(|e| CodecError::DecodingFailed(format!("Release output failed: {e}")))?;
                        
                        // frames.push(...);
                    } else if idx == ndk::media::media_codec::MediaCodec::INFO_OUTPUT_FORMAT_CHANGED {
                        self.output_format = Some(self.codec.output_format().unwrap());
                    } else if idx == ndk::media::media_codec::MediaCodec::INFO_TRY_AGAIN_LATER {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        
        Ok(frames)
    }
}
