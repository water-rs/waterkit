//! Apple VideoToolbox hardware encoding and decoding.

use objc2::rc::Retained;
use objc2_core_media::{
    kCMVideoCodecType_H264, kCMVideoCodecType_HEVC, CMSampleBuffer, CMTime,
};
use objc2_video_toolbox::{
    VTCompressionSession, VTCompressionSessionCreate, VTEncodeInfoFlags,
};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferLockBaseAddress,
    CVPixelBufferUnlockBaseAddress, CVPixelBufferGetBaseAddress,
    CVPixelBufferGetBytesPerRow, kCVPixelFormatType_32BGRA,
};
use crate::{VideoEncoder, VideoDecoder, CodecError, Frame, CodecType, PixelFormat};
use std::ptr;
use std::sync::{Arc, Mutex};
use std::ffi::c_void;
use std::ptr::NonNull;

/// Apple VideoToolbox hardware encoder.
pub struct AppleEncoder {
    session: Retained<VTCompressionSession>,
    output_buffer: Arc<Mutex<Vec<u8>>>,
    width: u32,
    height: u32,
    frame_count: i64,
}

unsafe impl Send for AppleEncoder {}
unsafe impl Sync for AppleEncoder {}

// C-callback for VideoToolbox - receives encoded data
unsafe extern "C-unwind" fn encode_callback(
    output_callback_ref_con: *mut c_void,
    _source_frame_ref_con: *mut c_void,
    status: i32,
    _info_flags: VTEncodeInfoFlags,
    sample_buffer: *mut CMSampleBuffer,
) {
    if status != 0 {
        eprintln!("VTCompressionSession callback error: {}", status);
        return;
    }
    
    if sample_buffer.is_null() {
        return;
    }
    
    // Extract encoded data from sample buffer using method-based API
    unsafe {
        let context = &*(output_callback_ref_con as *const Mutex<Vec<u8>>);
        
        // Use the method-based API: CMSampleBuffer::data_buffer()
        let sample_buf_ref = &*sample_buffer;
        if let Some(data_buffer) = sample_buf_ref.data_buffer() {
            let data_len = data_buffer.data_length();
            if data_len > 0 {
                let mut encoded_data = vec![0u8; data_len];
                let dest_ptr = NonNull::new(encoded_data.as_mut_ptr() as *mut c_void).unwrap();
                let result = data_buffer.copy_data_bytes(
                    0,
                    data_len,
                    dest_ptr,
                );
                
                if result == 0 {
                    if let Ok(mut lock) = context.lock() {
                        lock.extend_from_slice(&encoded_data);
                    }
                }
            }
        }
    }
}

impl AppleEncoder {
    /// Create a new Apple hardware encoder.
    pub fn new(codec: CodecType) -> Result<Self, CodecError> {
        Self::with_size(codec, 1920, 1080)
    }
    
    /// Create encoder with specific dimensions.
    pub fn with_size(codec: CodecType, width: u32, height: u32) -> Result<Self, CodecError> {
        let codec_type = match codec {
            CodecType::H264 => kCMVideoCodecType_H264,
            CodecType::H265 => kCMVideoCodecType_HEVC,
            _ => return Err(CodecError::Unsupported(format!("{:?}", codec))),
        };

        let output_buffer = Arc::new(Mutex::new(Vec::new()));
        let context_ptr = Arc::as_ptr(&output_buffer) as *mut c_void;
        
        let mut session_ptr: *mut VTCompressionSession = ptr::null_mut();
        
        unsafe {
            let status = VTCompressionSessionCreate(
                None, // allocator
                width as i32, 
                height as i32, 
                codec_type, 
                None, // encoderSpecification
                None, // sourceImageBufferAttributes
                None, // compressedDataAllocator
                Some(encode_callback),
                context_ptr,
                NonNull::new(&mut session_ptr).unwrap(),
            );
            
            if status != 0 {
                return Err(CodecError::InitializationFailed(format!("VT error: {}", status)));
            }
        }
        
        let session = unsafe { Retained::retain(session_ptr) }
            .ok_or(CodecError::InitializationFailed("Session null".into()))?;

        Ok(Self {
            session,
            output_buffer,
            width,
            height,
            frame_count: 0,
        })
    }
    
    /// Convert RGBA to BGRA (swap R and B channels).
    fn rgba_to_bgra(rgba: &[u8]) -> Vec<u8> {
        let mut bgra = rgba.to_vec();
        for chunk in bgra.chunks_exact_mut(4) {
            chunk.swap(0, 2);
        }
        bgra
    }
}

impl VideoEncoder for AppleEncoder {
    fn encode(&mut self, frame: &Frame) -> Result<Vec<u8>, CodecError> {
        // Validate dimensions
        if frame.width != self.width || frame.height != self.height {
            return Err(CodecError::EncodingFailed(format!(
                "Frame size {}x{} doesn't match encoder {}x{}", 
                frame.width, frame.height, self.width, self.height
            )));
        }
        
        // Convert to BGRA if needed (VideoToolbox prefers BGRA)
        let bgra_data = match frame.format {
            PixelFormat::Bgra => frame.data.as_ref().clone(),
            PixelFormat::Rgba => Self::rgba_to_bgra(&frame.data),
            _ => return Err(CodecError::Unsupported("Only RGBA/BGRA supported for Apple encoder".into())),
        };
        
        // Create CVPixelBuffer
        let mut pixel_buffer_ptr: *mut CVPixelBuffer = ptr::null_mut();
        unsafe {
            let status = CVPixelBufferCreate(
                None, // Use default allocator
                self.width as usize,
                self.height as usize,
                kCVPixelFormatType_32BGRA,
                None, // pixelBufferAttributes
                NonNull::new(&mut pixel_buffer_ptr).unwrap(),
            );
            
            if status != 0 || pixel_buffer_ptr.is_null() {
                return Err(CodecError::EncodingFailed(format!("CVPixelBufferCreate failed: {}", status)));
            }
        }
        
        // Get reference to pixel buffer
        let pixel_buffer = unsafe { &*pixel_buffer_ptr };
        
        // Lock and copy data to pixel buffer
        unsafe {
            use objc2_core_video::CVPixelBufferLockFlags;
            let lock_status = CVPixelBufferLockBaseAddress(pixel_buffer, CVPixelBufferLockFlags(0));
            if lock_status != 0 {
                return Err(CodecError::EncodingFailed(format!("CVPixelBufferLockBaseAddress failed: {}", lock_status)));
            }
            
            let base_addr = CVPixelBufferGetBaseAddress(pixel_buffer);
            let bytes_per_row = CVPixelBufferGetBytesPerRow(pixel_buffer);
            
            // Copy row by row (handle stride)
            let src_bytes_per_row = (self.width * 4) as usize;
            for row in 0..self.height as usize {
                let src_offset = row * src_bytes_per_row;
                let dst_offset = row * bytes_per_row;
                ptr::copy_nonoverlapping(
                    bgra_data.as_ptr().add(src_offset),
                    (base_addr as *mut u8).add(dst_offset),
                    src_bytes_per_row,
                );
            }
            
            CVPixelBufferUnlockBaseAddress(pixel_buffer, CVPixelBufferLockFlags(0));
        }
        
        // Clear output buffer for this frame
        if let Ok(mut lock) = self.output_buffer.lock() {
            lock.clear();
        }
        
        // Convert raw pointer to reference for encoding API
        let pixel_buffer_ref = unsafe { &*pixel_buffer };
        
        // Encode the frame using the session's method
        unsafe {
            use objc2_core_media::CMTimeFlags;
            
            // Create presentation time
            let presentation_time = CMTime { 
                value: self.frame_count, 
                timescale: 30, 
                flags: CMTimeFlags(1), 
                epoch: 0 
            };
            self.frame_count += 1;
            let duration = CMTime { 
                value: 1, 
                timescale: 30, 
                flags: CMTimeFlags(1), 
                epoch: 0 
            };
            
            // Use the method-based API
            let mut info_flags: VTEncodeInfoFlags = VTEncodeInfoFlags(0);
            let status = self.session.encode_frame(
                pixel_buffer_ref,
                presentation_time,
                duration,
                None, // frameProperties
                ptr::null_mut(), // sourceFrameRefCon
                &mut info_flags,
            );
            
            if status != 0 {
                return Err(CodecError::EncodingFailed(format!("encode_frame failed: {}", status)));
            }
            
            // Force completion
            let complete_time = CMTime { 
                value: i64::MAX, 
                timescale: 1, 
                flags: CMTimeFlags(1), 
                epoch: 0 
            };
            let complete_status = self.session.complete_frames(complete_time);
            
            if complete_status != 0 {
                return Err(CodecError::EncodingFailed(format!("complete_frames failed: {}", complete_status)));
            }
        }
        
        // Return encoded data
        let result = self.output_buffer.lock()
            .map(|lock| lock.clone())
            .map_err(|_| CodecError::Unknown("Lock error".into()))?;
        
        Ok(result)
    }
}

/// Apple VideoToolbox hardware decoder.
pub struct AppleDecoder {
    codec: CodecType,
}

unsafe impl Send for AppleDecoder {}
unsafe impl Sync for AppleDecoder {}

impl AppleDecoder {
    /// Create a new Apple hardware decoder.
    pub fn new(codec: CodecType) -> Result<Self, CodecError> {
        match codec {
            CodecType::H264 | CodecType::H265 => Ok(Self { codec }),
            _ => Err(CodecError::Unsupported(format!("{:?}", codec))),
        }
    }
}

impl VideoDecoder for AppleDecoder {
    fn decode(&mut self, _data: &[u8]) -> Result<Vec<Frame>, CodecError> {
        // VTDecompressionSession implementation would go here
        // This requires more complex setup with format descriptions
        Err(CodecError::Unsupported("VT decoding not yet implemented".into()))
    }
}
