//! Apple VideoToolbox hardware encoding and decoding.

use objc2::rc::Retained;
use objc2_core_media::{
    kCMVideoCodecType_H264, kCMVideoCodecType_HEVC, CMSampleBuffer, CMTime, CMBlockBuffer, CMSampleTimingInfo,
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
    context: Arc<EncoderContext>,
    width: u32,
    height: u32,
    frame_count: i64,
}

struct EncoderContext {
    encoded_data: Mutex<Vec<u8>>,
    codec_config: Mutex<Option<Vec<u8>>>,
}

unsafe impl Send for AppleEncoder {}
unsafe impl Sync for AppleEncoder {}

// C-callback for VideoToolbox - receives encoded data
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
    
    // Cast context
    let context = &*(output_callback_ref_con as *const EncoderContext);
    
    // Extract encoded data
    unsafe {
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
                    if let Ok(mut lock) = context.encoded_data.lock() {
                        lock.extend_from_slice(&encoded_data);
                    }
                }
            }
        }
        
        // Extract codec config if needed
        let need_config = if let Ok(lock) = context.codec_config.lock() {
            lock.is_none()
        } else {
            false
        };
        
        if need_config {
            #[link(name = "CoreMedia", kind = "framework")]
            #[link(name = "CoreFoundation", kind = "framework")]
            unsafe extern "C" {
                static kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms: *const c_void;
                static kCFAllocatorDefault: *const c_void;
                
                fn CMSampleBufferGetFormatDescription(sbuf: *mut CMSampleBuffer) -> *const c_void;
                fn CMFormatDescriptionGetExtension(desc: *const c_void, key: *const c_void) -> *const c_void;
                
                fn CFDictionaryGetValue(theDict: *const c_void, key: *const c_void) -> *const c_void;
                fn CFDataGetBytePtr(theData: *const c_void) -> *const u8;
                fn CFDataGetLength(theData: *const c_void) -> isize;
                
                fn CFStringCreateWithCString(alloc: *const c_void, cStr: *const i8, encoding: u32) -> *const c_void;
                fn CFRelease(cf: *const c_void);

                // Decoder FFI
                fn CMVideoFormatDescriptionCreate(
                    allocator: *const c_void,
                    codec_type: u32,
                    width: i32,
                    height: i32,
                    extensions: *const c_void, // CFDictionaryRef
                    format_description_out: *mut *const c_void // CMVideoFormatDescriptionRef
                ) -> i32;
                
                fn CFDictionaryCreate(
                    allocator: *const c_void,
                    keys: *const *const c_void,
                    values: *const *const c_void,
                    numValues: isize,
                    keyCallBacks: *const c_void,
                    valueCallBacks: *const c_void
                ) -> *const c_void; // CFDictionaryRef
                
                fn CFDataCreate(
                    allocator: *const c_void,
                    bytes: *const u8,
                    length: isize
                ) -> *const c_void; // CFDataRef
                
                static kCFTypeDictionaryKeyCallBacks: c_void;
                static kCFTypeDictionaryValueCallBacks: c_void;
                
                fn VTDecompressionSessionCreate(
                    allocator: *const c_void,
                    format_description: *const c_void,
                    decoder_specification: *const c_void,
                    image_buffer_attributes: *const c_void,
                    output_callback: *const c_void, // VTDecompressionOutputCallbackRecord
                    decompression_session_out: *mut *mut c_void // VTDecompressionSessionRef
                ) -> i32;
                
                fn VTDecompressionSessionDecodeFrame(
                    session: *mut c_void,
                    sample_buffer: *mut CMSampleBuffer,
                    flags: u32,
                    source_frame_ref_con: *mut c_void,
                    info_flags_out: *mut u32
                ) -> i32;
                
                fn VTDecompressionSessionWaitForAsynchronousFrames(session: *mut c_void) -> i32;
                
                fn CMSampleBufferCreate(
                     allocator: *const c_void,
                     data_buffer: *const c_void, // CMBlockBufferRef (we need this!)
                     data_ready: u8,
                     make_data_ready_callback: *const c_void,
                     make_data_ready_ref_con: *mut c_void,
                     format_description: *const c_void,
                     sample_count: isize,
                     sample_timing_entry_count: isize,
                     sample_timing_array: *const c_void, // CMSampleTimingInfo
                     sample_size_entry_count: isize,
                     sample_size_array: *const usize,
                     sample_buffer_out: *mut *mut CMSampleBuffer
                ) -> i32;
                
                fn CMBlockBufferCreateWithMemoryBlock(
                    structureAllocator: *const c_void,
                    memoryBlock: *mut c_void,
                    blockLength: usize,
                    blockAllocator: *const c_void,
                    customBlockSource: *const c_void,
                    offsetToData: usize,
                    dataLength: usize,
                    flags: u32,
                    blockBufferOut: *mut *const c_void // CMBlockBufferRef
                ) -> i32;

                fn CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
                    videoDesc: *const c_void,
                    parameterSetIndex: usize,
                    parameterSetPointerOut: *mut *const u8,
                    parameterSetSizeOut: *mut usize,
                    parameterSetCountOut: *mut usize,
                    nalUnitHeaderLengthOut: *mut i32
                ) -> i32;
                
                fn CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                    videoDesc: *const c_void,
                    parameterSetIndex: usize,
                    parameterSetPointerOut: *mut *const u8,
                    parameterSetSizeOut: *mut usize,
                    parameterSetCountOut: *mut usize,
                    nalUnitHeaderLengthOut: *mut i32
                ) -> i32;
            }

            fn construct_hevc_config(format_desc: *const c_void) -> Option<Vec<u8>> {
                unsafe {
                    let mut parameter_set_count = 0;
                    // Get count first (index 0, pointers null)
                    // Actually GetHEVCParameterSetAtIndex(..., ptr::null_mut(), ...) doesn't return count of ALL sets usually, 
                    // it returns info for specific index.
                    // But we can verify existence by looping until error.
                    // Or we can query index 0?
                    // Docs imply we just loop.
                    
                    let mut vps_list = Vec::new();
                    let mut sps_list = Vec::new();
                    let mut pps_list = Vec::new();
                    
                    let mut index = 0;
                    loop {
                        let mut ptr: *const u8 = ptr::null();
                        let mut size: usize = 0;
                        let mut header_len: i32 = 0;
                        
                        let status = CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
                            format_desc,
                            index,
                            &mut ptr,
                            &mut size,
                            ptr::null_mut(),
                            &mut header_len
                        );
                        
                        if status != 0 {
                            eprintln!("GetHEVCParameterSetAtIndex failed at index {}: {}", index, status);
                            break;
                        }
                        
                        let data = std::slice::from_raw_parts(ptr, size).to_vec();
                        eprintln!("Found HEVC NAL at index {}: len={}, type={}", index, size, (data[0] >> 1) & 0x3F);
                        
                        // Parse NAL type
                        // HEVC NAL header is 2 bytes.
                        // Type is bits 1-6 of first byte. (Forbidden bit 0, Type 6 bits, LayerId 6 bits, TemporalId 3 bits)
                        // Byte 0: F(1) Type(6) LayerId_high(1)
                        // Byte 1: LayerId_low(5) TemporalId(3)
                        // Type = (data[0] >> 1) & 0x3F.
                        if data.len() > 2 {
                            let nal_type = (data[0] >> 1) & 0x3F;
                            match nal_type {
                                32 => vps_list.push(data),
                                33 => sps_list.push(data),
                                34 => pps_list.push(data),
                                _ => {}
                            }
                        }
                        
                        index += 1;
                    }
                    
                    if vps_list.is_empty() && sps_list.is_empty() && pps_list.is_empty() {
                        return None;
                    }
                    
                    // Construct hvcC
                    // Ref: ISO 14496-15
                    let mut config = Vec::new();
                    
                    // Header
                    config.push(1); // version
                    
                    // Header fields from SPS
                    if let Some(sps) = sps_list.first() {
                         // SPS payload starts after 2 byte header.
                         // But we also skip 1 byte (max sub layers etc)
                         // Then 12 bytes profile_tier_level
                         if sps.len() > 15 {
                             let payload = &sps[2..];
                             // Skip 1 byte (video_param_set_id stuff usually? No, max_sub_layers_minus1 etc)
                             // Actually, let's look at profile_tier_level structure in SPS.
                             // It starts at byte 1 of payload (after max_sub_layers byte) IF sps_max_sub_layers_minus1 is there.
                             // For simple profile, yes.
                             config.extend_from_slice(&payload[1..13]); // 12 bytes profile/tier/level/constraints
                         } else {
                             // Fallback defaults
                             config.extend_from_slice(&[0; 12]); 
                         }
                    } else {
                         config.extend_from_slice(&[0; 12]);
                    }
                    
                    config.push(0); // min_spatial_segmentation_idc (upper)
                    config.push(0); // min_spatial_segmentation_idc (lower)
                    config.push(0); // parallelismType
                    config.push(1); // chromaFormat (4:2:0 = 1)
                    config.push(0); // bitDepthLumaMinus8
                    config.push(0); // bitDepthChromaMinus8
                    config.push(0); // avgFrameRate (upper)
                    config.push(0); // avgFrameRate (lower)
                    
                    // constFrameRate(2bit), numTemporalLayers(3bit), temporalIdNested(1bit), lengthSizeMinusOne(2bit)
                    // lengthSizeMinusOne = 3 (4 bytes) -> 0x03.
                    // numTemporalLayers = 1 -> 0x08? 
                    // Let's use 0x0F (nested + lengthSize=3)
                    config.push(0x83); // temporalIdNested=1, lengthSizeMinusOne=3. 
                    
                    // NumArrays
                    let num_arrays = (if !vps_list.is_empty() { 1 } else { 0 }) +
                                     (if !sps_list.is_empty() { 1 } else { 0 }) +
                                     (if !pps_list.is_empty() { 1 } else { 0 });
                    config.push(num_arrays as u8);
                    
                    // Arrays
                    let mut write_array = |nal_type: u8, list: &Vec<Vec<u8>>| {
                        if list.is_empty() { return; }
                        // Array header: completeness(1bit), reserved(1bit), nal_unit_type(6bits)
                        config.push(0x80 | (nal_type & 0x3F)); // completeness=1
                        
                        // Num NALUs
                        let count = list.len() as u16;
                        config.push((count >> 8) as u8);
                        config.push((count & 0xFF) as u8);
                        
                        for nal in list {
                            let len = nal.len() as u16;
                            config.push((len >> 8) as u8);
                            config.push((len & 0xFF) as u8);
                            config.extend_from_slice(nal);
                        }
                    };
                    
                    write_array(32, &vps_list); // VPS
                    write_array(33, &sps_list); // SPS
                    write_array(34, &pps_list); // PPS
                    
                    Some(config)
                }
            }
            
            fn construct_avc_config(format_desc: *const c_void) -> Option<Vec<u8>> {
                 unsafe {
                     // Get SPS and PPS
                     let mut sps_list = Vec::new();
                     let mut pps_list = Vec::new();
                     
                     let mut index = 0;
                     loop {
                         let mut ptr: *const u8 = ptr::null();
                         let mut size: usize = 0;
                         let mut header_len: i32 = 0;
                         
                         let status = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                             format_desc,
                             index,
                             &mut ptr,
                             &mut size,
                             ptr::null_mut(),
                             &mut header_len
                         );
                         
                         if status != 0 {
                             break;
                         }
                         
                         let data = std::slice::from_raw_parts(ptr, size).to_vec();
                         // H.264 NAL header 1 byte. Type low 5 bits.
                         // SPS=7, PPS=8
                         if data.len() > 1 {
                             let nal_type = data[0] & 0x1F;
                             match nal_type {
                                 7 => sps_list.push(data),
                                 8 => pps_list.push(data),
                                 _ => {}
                             }
                         }
                         index += 1;
                     }

                     if sps_list.is_empty() && pps_list.is_empty() {
                         return None;
                     }
                     
                     // Construct avcC
                     let mut config = Vec::new();
                     config.push(1); // version
                     
                     if let Some(sps) = sps_list.first() {
                         if sps.len() > 3 {
                             config.push(sps[1]); // profile
                             config.push(sps[2]); // compat
                             config.push(sps[3]); // level
                         } else {
                             config.extend_from_slice(&[0, 0, 0]);
                         }
                     } else {
                         config.extend_from_slice(&[0, 0, 0]);
                     }
                     
                     config.push(0xFF); // 111111 + lengthSizeMinusOne(3)
                     
                     // Num SPS
                     config.push(0xE0 | (sps_list.len() as u8 & 0x1F));
                     for sps in &sps_list {
                         let len = sps.len() as u16;
                         config.push((len >> 8) as u8);
                         config.push((len & 0xFF) as u8);
                         config.extend_from_slice(sps);
                     }
                     
                     // Num PPS
                     config.push(pps_list.len() as u8);
                     for pps in &pps_list {
                         let len = pps.len() as u16;
                         config.push((len >> 8) as u8);
                         config.push((len & 0xFF) as u8);
                         config.extend_from_slice(pps);
                     }
                     
                     Some(config)
                 }
            }
            
            let format_desc = CMSampleBufferGetFormatDescription(sample_buffer);
            if !format_desc.is_null() {
                // First try standard extension lookup (cheap)
                let atoms_key = kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms;
                let atoms = CMFormatDescriptionGetExtension(format_desc, atoms_key);
                let mut found_config = false;
                
                if !atoms.is_null() {
                     // ... existing atomic extraction code ...
                     // create "hvcC" string
                     let hvc_c_str = b"hvcC\0";
                     let avc_c_str = b"avcC\0"; // Check both or match?
                     // We should know codec type from somewhere, but here we can try both or check specific
                     // Ideally we check codec info.
                     // For now, let's focus on hvcC replacement logic.
                     
                     let key_str = CFStringCreateWithCString(
                         kCFAllocatorDefault, 
                         hvc_c_str.as_ptr() as *const i8, 
                         0x08000100 
                     );
                     
                     if !key_str.is_null() {
                         let hvc_data = CFDictionaryGetValue(atoms, key_str);
                         if !hvc_data.is_null() {
                             let len = CFDataGetLength(hvc_data);
                             let ptr = CFDataGetBytePtr(hvc_data);
                             if len > 20 && !ptr.is_null() { // Basic check: > 20 bytes for HEVC
                                 let config_bytes = std::slice::from_raw_parts(ptr, len as usize).to_vec();
                                 if let Ok(mut lock) = context.codec_config.lock() {
                                     eprintln!("Found atomic hvcC extension with size {}: {:02X?}", len, config_bytes);
                                     *lock = Some(config_bytes);
                                     found_config = true;
                                 }
                             } else {
                                 eprintln!("Ignored atomic hvcC extension with size {}", len);
                             }
                         }
                         CFRelease(key_str);
                     }
                }
                
                if !found_config {
                     eprintln!("Attempting manual properties extraction...");
                     // Try manual construction
                     let manual_config = construct_hevc_config(format_desc);
                     if let Some(config) = manual_config {
                         if let Ok(mut lock) = context.codec_config.lock() {
                             *lock = Some(config);
                              // println!("Constructed Manual HEVC Config: {} bytes", lock.as_ref().unwrap().len());
                         }
                     } else {
                         // Try AVC
                         let manual_avc = construct_avc_config(format_desc);
                         if let Some(config) = manual_avc {
                            if let Ok(mut lock) = context.codec_config.lock() {
                                 *lock = Some(config);
                            }
                         }
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

        let context = Arc::new(EncoderContext {
            encoded_data: Mutex::new(Vec::new()),
            codec_config: Mutex::new(None),
        });
        let context_ptr = Arc::as_ptr(&context) as *mut c_void;
        
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
            .ok_or(CodecError::InitializationFailed("Failed to retain session".into()))?;

        Ok(Self {
            session,
            context,
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
    
    /// Encode directly from IOSurface pointer (zero-copy from ScreenCaptureKit).
    /// 
    /// This method takes an IOSurface pointer and creates a CVPixelBuffer from it,
    /// allowing VideoToolbox to encode directly from GPU memory without any CPU copy.
    pub fn encode_iosurface(&mut self, iosurface_ptr: u64) -> Result<Vec<u8>, CodecError> {
        // FFI binding for CVPixelBufferCreateWithIOSurface
        #[link(name = "CoreVideo", kind = "framework")]
        unsafe extern "C" {
            fn CVPixelBufferCreateWithIOSurface(
                allocator: *const c_void,
                surface: *const c_void,  // IOSurfaceRef
                pixel_buffer_attributes: *const c_void,
                pixel_buffer_out: *mut *mut CVPixelBuffer,
            ) -> i32;
        }
        
        if iosurface_ptr == 0 {
            return Err(CodecError::EncodingFailed("NULL IOSurface pointer".into()));
        }
        
        // Create CVPixelBuffer from IOSurface (zero-copy)
        let mut pixel_buffer_ptr: *mut CVPixelBuffer = ptr::null_mut();
        unsafe {
            let status = CVPixelBufferCreateWithIOSurface(
                ptr::null(),  // default allocator
                iosurface_ptr as *const c_void,  // IOSurfaceRef
                ptr::null(),  // pixelBufferAttributes
                &mut pixel_buffer_ptr,
            );
            
            if status != 0 || pixel_buffer_ptr.is_null() {
                return Err(CodecError::EncodingFailed(
                    format!("CVPixelBufferCreateWithIOSurface failed: {}", status)
                ));
            }
        }
        
        // Clear output buffer
        if let Ok(mut lock) = self.context.encoded_data.lock() {
            lock.clear();
        }
        
        // Encode the frame
        let pixel_buffer_ref = unsafe { &*pixel_buffer_ptr };
        
        unsafe {
            use objc2_core_media::CMTimeFlags;
            
            let presentation_time = CMTime {
                value: self.frame_count,
                timescale: 30,
                flags: CMTimeFlags(1),
                epoch: 0,
            };
            self.frame_count += 1;
            
            let duration = CMTime {
                value: 1,
                timescale: 30,
                flags: CMTimeFlags(1),
                epoch: 0,
            };
            
            let mut info_flags = VTEncodeInfoFlags(0);
            let status = self.session.encode_frame(
                pixel_buffer_ref,
                presentation_time,
                duration,
                None,
                ptr::null_mut(),
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
                epoch: 0,
            };
            let complete_status = self.session.complete_frames(complete_time);
            
            if complete_status != 0 {
                return Err(CodecError::EncodingFailed(
                    format!("complete_frames failed: {}", complete_status)
                ));
            }
        }
        
        // Return encoded data
        let result = self.context.encoded_data.lock()
            .map(|lock| lock.clone())
            .map_err(|_| CodecError::Unknown("Lock error".into()))?;
        
        Ok(result)
    }
    
    /// Get the codec configuration data (e.g. hvcC or avcC atom) if available.
    pub fn get_codec_config(&self) -> Option<Vec<u8>> {
        if let Ok(lock) = self.context.codec_config.lock() {
            lock.clone()
        } else {
            None
        }
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
    if let Ok(mut lock) = self.context.encoded_data.lock() {
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
    let result = self.context.encoded_data.lock()
        .map(|lock| lock.clone())
        .map_err(|_| CodecError::Unknown("Lock error".into()))?;
    
    Ok(result)

}
}

// FFI Definitions for Decoder
#[link(name = "CoreMedia", kind = "framework")]
#[link(name = "VideoToolbox", kind = "framework")]
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CMVideoFormatDescriptionCreate(
        allocator: *const c_void,
        codec_type: u32,
        width: i32,
        height: i32,
        extensions: *const c_void, // CFDictionaryRef
        format_description_out: *mut *const c_void // CMVideoFormatDescriptionRef
    ) -> i32;
    
    fn CFDictionaryCreate(
        allocator: *const c_void,
        keys: *const *const c_void,
        values: *const *const c_void,
        numValues: isize,
        keyCallBacks: *const c_void,
        valueCallBacks: *const c_void
    ) -> *const c_void; // CFDictionaryRef
    
    fn CFDataCreate(
        allocator: *const c_void,
        bytes: *const u8,
        length: isize
    ) -> *const c_void; // CFDataRef
    
    fn CFRelease(cf: *const c_void); // Re-declared here for decoder usage
    fn CFStringCreateWithCString(alloc: *const c_void, cStr: *const i8, encoding: u32) -> *const c_void; // Re-declared

    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    static kCFAllocatorDefault: *const c_void;
    
    fn VTDecompressionSessionCreate(
        allocator: *const c_void,
        format_description: *const c_void,
        decoder_specification: *const c_void,
        image_buffer_attributes: *const c_void,
        output_callback: *const c_void, // VTDecompressionOutputCallbackRecord
        decompression_session_out: *mut *mut c_void // VTDecompressionSessionRef
    ) -> i32;
    
    fn VTDecompressionSessionDecodeFrame(
        session: *mut c_void,
        sample_buffer: *mut CMSampleBuffer,
        flags: u32,
        source_frame_ref_con: *mut c_void,
        info_flags_out: *mut u32
    ) -> i32;
    
    fn VTDecompressionSessionWaitForAsynchronousFrames(session: *mut c_void) -> i32;
    fn VTDecompressionSessionInvalidate(session: *mut c_void);
    
    fn CMBlockBufferCreateWithMemoryBlock(
        structureAllocator: *const c_void,
        memoryBlock: *mut c_void,
        blockLength: usize,
        blockAllocator: *const c_void,
        customBlockSource: *const c_void,
        offsetToData: usize,
        dataLength: usize,
        flags: u32,
        blockBufferOut: *mut *const c_void // CMBlockBufferRef
    ) -> i32;
    
    fn CMSampleBufferCreate(
        allocator: *const c_void,
        data_buffer: *const c_void, // CMBlockBufferRef
        data_ready: u8,
        make_data_ready_callback: *const c_void,
        make_data_ready_ref_con: *mut c_void,
        format_description: *const c_void,
        sample_count: isize,
        sample_timing_entry_count: isize,
        sample_timing_array: *const c_void, // CMSampleTimingInfo
        sample_size_entry_count: isize,
        sample_size_array: *const usize,
        sample_buffer_out: *mut *mut CMSampleBuffer
    ) -> i32;
}

#[repr(C)]
struct VTDecompressionOutputCallbackRecord {
    decompression_output_callback: extern "C" fn(
        *mut c_void,
        *mut c_void,
        i32,
        u32, // VTDecodeInfoFlags
        *mut CVPixelBuffer, // CVImageBufferRef
        CMTime,
        CMTime
    ),
    decompression_output_ref_con: *mut c_void,
}

struct DecoderContext {
    decoded_frames: Mutex<Vec<Frame>>,
    width: u32,
    height: u32,
}

/// Apple VideoToolbox hardware decoder.
pub struct AppleDecoder {
    #[allow(dead_code)]
    codec: CodecType,
    session: *mut c_void, // VTDecompressionSessionRef
    context: Arc<DecoderContext>,
    format_desc: *const c_void, // CMVideoFormatDescriptionRef
}

unsafe impl Send for AppleDecoder {}
unsafe impl Sync for AppleDecoder {}

extern "C" fn decode_callback(
    decompression_output_ref_con: *mut c_void,
    _source_frame_ref_con: *mut c_void,
    status: i32,
    _info_flags: u32,
    image_buffer: *mut CVPixelBuffer,
    _presentation_time_stamp: CMTime,
    _presentation_duration: CMTime,
) {

    if status != 0 {
        eprintln!("VTDecompressionSession callback error: {}", status);
        return;
    }
    
    if image_buffer.is_null() {
        return;
    }
    
    let context = unsafe { &*(decompression_output_ref_con as *const DecoderContext) };
    
    // Convert raw pointer to reference for objc2 APIs
    let image_buffer_ref = unsafe { &*image_buffer };

    unsafe {
        use objc2_core_video::{CVPixelBufferLockFlags, CVPixelBufferGetPixelFormatType};
        
        // Check actual pixel format
        let pixel_format = CVPixelBufferGetPixelFormatType(image_buffer_ref);
        // kCVPixelFormatType_32BGRA = 0x42475241 = 'BGRA'
        // kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange = 0x34323076 = '420v' (NV12)
        // kCVPixelFormatType_420YpCbCr8BiPlanarFullRange = 0x34323066 = '420f'
        let is_bgra = pixel_format == 0x42475241;
        let is_nv12 = pixel_format == 0x34323076 || pixel_format == 0x34323066;
        

        
        // Lock base address
        if CVPixelBufferLockBaseAddress(image_buffer_ref, CVPixelBufferLockFlags(0)) == 0 {
            let width = objc2_core_video::CVPixelBufferGetWidth(image_buffer_ref);
            let height = objc2_core_video::CVPixelBufferGetHeight(image_buffer_ref);
            let bytes_per_row = CVPixelBufferGetBytesPerRow(image_buffer_ref);
            let base_addr = CVPixelBufferGetBaseAddress(image_buffer_ref);
            
            if !base_addr.is_null() {
                // Crop dimensions to expected size (handling padding)
                let expected_width = unsafe { (*(decompression_output_ref_con as *const DecoderContext)).width };
                let expected_height = unsafe { (*(decompression_output_ref_con as *const DecoderContext)).height };
                
                // Ensure we don't read out of bounds
                let copy_width = (width as u32).min(expected_width);
                let copy_height = (height as u32).min(expected_height);
                
                let mut data = Vec::with_capacity((copy_width * copy_height * 4) as usize);
                
                if is_bgra {
                    // Direct copy for BGRA
                    let src_stride = bytes_per_row;
                    for row in 0..copy_height {
                        let src = (base_addr as *const u8).add(row as usize * src_stride);
                        let row_slice = std::slice::from_raw_parts(src, (copy_width * 4) as usize);
                        data.extend_from_slice(row_slice);
                    }
                } else if is_nv12 {
                    // NV12 to BGRA conversion
                    // NV12 format: Y plane followed by interleaved UV plane
                    use objc2_core_video::{CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRowOfPlane};
                    let y_plane = CVPixelBufferGetBaseAddressOfPlane(image_buffer_ref, 0) as *const u8;
                    let uv_plane = CVPixelBufferGetBaseAddressOfPlane(image_buffer_ref, 1) as *const u8;
                    let y_stride = CVPixelBufferGetBytesPerRowOfPlane(image_buffer_ref, 0);
                    let uv_stride = CVPixelBufferGetBytesPerRowOfPlane(image_buffer_ref, 1);
                    
                    for row in 0..copy_height {
                        let r = row as usize;
                        for col in 0..copy_width {
                            let c = col as usize;
                            let y = *y_plane.add(r * y_stride + c) as i32;
                            let u = *uv_plane.add((r / 2) * uv_stride + (c / 2) * 2) as i32;
                            let v = *uv_plane.add((r / 2) * uv_stride + (c / 2) * 2 + 1) as i32;
                            
                            // YUV to RGB conversion (BT.601)
                            let val_c = y - 16;
                            let val_d = u - 128;
                            let val_e = v - 128;
                            
                            let red = ((298 * val_c + 409 * val_e + 128) >> 8).clamp(0, 255) as u8;
                            let green = ((298 * val_c - 100 * val_d - 208 * val_e + 128) >> 8).clamp(0, 255) as u8;
                            let blue = ((298 * val_c + 516 * val_d + 128) >> 8).clamp(0, 255) as u8;
                            
                            // BGRA order
                            data.push(blue);
                            data.push(green);
                            data.push(red);
                            data.push(255); // Alpha
                        }
                    }
                } else {
                    eprintln!("Unsupported pixel format: 0x{:X}", pixel_format);
                }
                
                if !data.is_empty() {
                    let frame = Frame {
                        data: Arc::new(data),
                        width: copy_width,
                        height: copy_height,
                        format: PixelFormat::Bgra,
                        timestamp_ns: 0,
                    };
                    
                    if let Ok(mut frames) = context.decoded_frames.lock() {
                        frames.push(frame);
                    }
                }
            }
            CVPixelBufferUnlockBaseAddress(image_buffer_ref, CVPixelBufferLockFlags(0));
        }
    }
}

impl AppleDecoder {
    /// Create a new Apple hardware decoder.
    pub fn new(codec: CodecType, config: Option<&[u8]>, width: u32, height: u32) -> Result<Self, CodecError> {
        if config.is_none() {
             return Err(CodecError::InitializationFailed("Codec config (hvcC/avcC) required".into()));
        }
        let mut config_bytes = config.unwrap();
        
        let codec_type = match codec {
            CodecType::H264 => kCMVideoCodecType_H264,
            CodecType::H265 => kCMVideoCodecType_HEVC,
            _ => return Err(CodecError::Unsupported(format!("{:?}", codec))),
        };
        
        // Strip Box Header (size + type) if present. 
        // MP4/MOV files usually provide the full box (e.g. 00 00 00 23 avcC ...).
        // VT requires just the payload (AVCDecoderConfigurationRecord).
        if config_bytes.len() > 8 {
            let atom_key = match codec {
                CodecType::H264 => b"avcC",
                CodecType::H265 => b"hvcC",
                _ => b"????",
            };
            if &config_bytes[4..8] == atom_key {
                // Strip box header (8 bytes: size + type)
                config_bytes = &config_bytes[8..];
            }
        }
        
        let context = Arc::new(DecoderContext {
            decoded_frames: Mutex::new(Vec::new()),
            width,
            height,
        });
        
        unsafe {
             // 1. Create Format Description from Atom (config)
             // Create "hvcC" or "avcC" key
             let atom_key_str = if codec == CodecType::H265 { b"hvcC\0" } else { b"avcC\0" };
             let key_cf = CFStringCreateWithCString(kCFAllocatorDefault, atom_key_str.as_ptr() as *const i8, 0x08000100);
             
             // Create Data
             let data_cf = CFDataCreate(kCFAllocatorDefault, config_bytes.as_ptr(), config_bytes.len() as isize);
             
             // Create Dictionary { "hvcC": <data> }
             let keys = [key_cf];
             let values = [data_cf];
             let atoms_dict = CFDictionaryCreate(
                 kCFAllocatorDefault,
                 keys.as_ptr() as *const *const c_void,
                 values.as_ptr() as *const *const c_void,
                 1,
                 &kCFTypeDictionaryKeyCallBacks,
                 &kCFTypeDictionaryValueCallBacks
             );
             
             // Create Extensions Dictionary { "SampleDescriptionExtensionAtoms": <atoms_dict> }
             // We need kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms key.
             // It's a string key.
             let ext_key_str = b"SampleDescriptionExtensionAtoms\0"; 
             let ext_key_cf = CFStringCreateWithCString(kCFAllocatorDefault, ext_key_str.as_ptr() as *const i8, 0x08000100);
             
             let ext_keys = [ext_key_cf];
             let ext_values = [atoms_dict];
             let extensions = CFDictionaryCreate(
                 kCFAllocatorDefault,
                 ext_keys.as_ptr() as *const *const c_void,
                 ext_values.as_ptr() as *const *const c_void,
                 1,
                 &kCFTypeDictionaryKeyCallBacks,
                 &kCFTypeDictionaryValueCallBacks
             );
             
             let mut format_desc: *const c_void = ptr::null();
             let status = CMVideoFormatDescriptionCreate(
                 kCFAllocatorDefault,
                 codec_type,
                 1920, 1080, // Dimensions will be derived from SPS if possible, but we must pass something. 
                 // If using extensions, this acts as hint or is overridden?
                 extensions,
                 &mut format_desc
             );
             
             CFRelease(key_cf);
             CFRelease(data_cf);
             CFRelease(atoms_dict);
             CFRelease(ext_key_cf);
             CFRelease(extensions);
             
             if status != 0 {
                 return Err(CodecError::InitializationFailed(format!("CMVideoFormatDescriptionCreate failed: {}", status)));
             }
             
             // 2. Create Decompression Session
             let callback_record = VTDecompressionOutputCallbackRecord {
                 decompression_output_callback: decode_callback,
                 decompression_output_ref_con: Arc::as_ptr(&context) as *mut c_void,
             };
             
             // Image buffer attributes: request BGRA output
             // kCVPixelFormatType_32BGRA = 'BGRA' = 0x42475241
             let pixel_format_bgra: u32 = 0x42475241;
             
             #[link(name = "CoreFoundation", kind = "framework")]
             unsafe extern "C" {
                 fn CFNumberCreate(allocator: *const c_void, theType: i64, valuePtr: *const c_void) -> *const c_void;
             }
             
             // kCFNumberSInt32Type = 3
             let pixel_format_number = CFNumberCreate(
                 kCFAllocatorDefault,
                 3, // kCFNumberSInt32Type
                 &pixel_format_bgra as *const u32 as *const c_void
             );
             
             // Use proper kCVPixelBufferPixelFormatTypeKey from objc2
             use objc2_core_video::kCVPixelBufferPixelFormatTypeKey;
             let pixel_format_key: *const c_void = kCVPixelBufferPixelFormatTypeKey as *const _ as *const c_void;
             
             // Create attributes dictionary
             let attr_keys = [pixel_format_key];
             let attr_values = [pixel_format_number as *const c_void];
             let image_buffer_attrs = CFDictionaryCreate(
                 kCFAllocatorDefault,
                 attr_keys.as_ptr(),
                 attr_values.as_ptr(),
                 1,
                 &kCFTypeDictionaryKeyCallBacks,
                 &kCFTypeDictionaryValueCallBacks
             );
             
             let mut session: *mut c_void = ptr::null_mut();
             let status = VTDecompressionSessionCreate(
                 kCFAllocatorDefault,
                 format_desc,
                 ptr::null(), // decoder specification
                 image_buffer_attrs, // request BGRA output
                 &callback_record as *const _ as *const c_void,
                 &mut session
             );
             
             // Don't release pixel_format_key - it's a static constant
             CFRelease(pixel_format_number);
             CFRelease(image_buffer_attrs);
             
             if status != 0 {
                 CFRelease(format_desc);
                 return Err(CodecError::InitializationFailed(format!("VTDecompressionSessionCreate failed: {}", status)));
             }
             
             Ok(Self {
                 codec,
                 session,
                 context,
                 format_desc
             })
        }
    }
}

impl Drop for AppleDecoder {
    fn drop(&mut self) {
        unsafe {
            if !self.session.is_null() {
                VTDecompressionSessionInvalidate(self.session);
                CFRelease(self.session);
            }
            if !self.format_desc.is_null() {
                CFRelease(self.format_desc);
            }
        }
    }
}

impl AppleDecoder {
    pub fn decode(&mut self, data: &[u8], pts: u64, timescale: u32) -> Result<Vec<Frame>, CodecError> {
        if data.len() < 4 {
             return Err(CodecError::DecodingFailed("Data too short".into()));
        }

        let mut frames = Vec::new();
        
        unsafe {
            // 1. Create CMBlockBuffer wrapping the data
            // We must copy data because `data` slice lifetime is short.
            // CMBlockBufferCreateWithMemoryBlock with NULL allocates memory.
            let mut block_buffer: *const c_void = ptr::null();
            
            // Allocate memory manually to ensure alignment and ownership passing?
            // Actually, let's use a simpler approach: 
            // Create empty BlockBuffer, then ReplaceDataBytes?
            // Or use `CMBlockBufferCreateWithMemoryBlock` passing NULL as memoryBlock causing it to allocate.
            let status = CMBlockBufferCreateWithMemoryBlock(
                kCFAllocatorDefault,
                ptr::null_mut(),
                data.len(),
                kCFAllocatorDefault,
                ptr::null(),
                0,
                data.len(),
                0, // kCMBlockBufferAssureMemoryNow
                &mut block_buffer
            );
            
            if status != 0 {
                return Err(CodecError::DecodingFailed(format!("CMBlockBufferCreate failed: {}", status)));
            }
            
            // Now copy data into it.
             #[link(name = "CoreMedia", kind = "framework")]
             unsafe extern "C" {
                 fn CMBlockBufferReplaceDataBytes(
                     sourceBytes: *const c_void,
                     destinationBuffer: *const c_void,
                     offsetIntoDestination: usize,
                     dataLength: usize
                 ) -> i32;
             }
             
             let status = CMBlockBufferReplaceDataBytes(
                 data.as_ptr() as *const c_void,
                 block_buffer,
                 0,
                 data.len()
             );
             
             if status != 0 {
                  CFRelease(block_buffer);
                  return Err(CodecError::DecodingFailed(format!("CMBlockBufferReplaceDataBytes failed: {}", status)));
             }
            
            // 2. Create CMSampleBuffer
            let mut sample_buffer: *mut CMSampleBuffer = ptr::null_mut();
            // Create timing info
            // Create timing info
            let pts_time = CMTime {
                value: pts as i64,
                timescale: timescale as i32,
                flags: objc2_core_media::CMTimeFlags(1), // Valid
                epoch: 0,
            };
            let invalid_time = CMTime {
                value: 0,
                timescale: 0,
                flags: objc2_core_media::CMTimeFlags(0),
                epoch: 0,
            };

            let timing_info = CMSampleTimingInfo {
                duration: invalid_time,
                presentationTimeStamp: pts_time,
                decodeTimeStamp: invalid_time,
            };

            let status = CMSampleBufferCreate(
                kCFAllocatorDefault,
                block_buffer,
                1, // dataReady
                ptr::null(),
                ptr::null_mut(),
                self.format_desc,
                1, // sampleCount
                1, &timing_info as *const _ as *const c_void, // timing
                0, ptr::null(), // size (default)
                &mut sample_buffer
            );
             
            CFRelease(block_buffer); // CMSampleBuffer retains it
            
            if status != 0 {
                return Err(CodecError::DecodingFailed(format!("CMSampleBufferCreate failed: {}", status)));
            }
            
            // 3. Decode Frame (Async but we wait? Or sync if flags set?)
            // kVTDecodeFrame_EnableAsynchronousDecompression = 1<<0
            // We pass 0 to try synchronous? No, VT often forces async.
            let flags = 0; // Synchronous if possible?
            let mut info_flags = 0;
            
            let status = VTDecompressionSessionDecodeFrame(
                self.session,
                sample_buffer,
                flags, // 0 usually means synchronous if allowed
                ptr::null_mut(),
                &mut info_flags
            );
            
            if status != 0 {
                eprintln!("VTDecompressionSessionDecodeFrame failed: {}", status);
                CFRelease(sample_buffer as *const c_void);
                return Err(CodecError::DecodingFailed(format!("Decode failed: {}", status)));
            }
            
            // CFRelease(sample_buffer); // leak? CMSampleBuffer is CFType.
            // CMSampleBuffer is typedef for opaque struct. We need to release it.
            // But we used `*mut CMSampleBuffer` which is `CMSampleBufferRef`.
            CFRelease(sample_buffer as *const c_void);
            
            if status != 0 {
                return Err(CodecError::DecodingFailed(format!("VTDecodeFrame failed: {}", status)));
            }
            
            // If async, we should wait.
            // VTDecompressionSessionWaitForAsynchronousFrames(self.session);
            // Since we passed 0 flags and callback is likely called on another thread or same thread?
            // If synchronous, callback is called before return.
            // Let's assume sync for now.
        }
        
        // Collect frames
        if let Ok(mut lock) = self.context.decoded_frames.lock() {
            frames.append(&mut lock);
        }
        
        Ok(frames)
    }
}
