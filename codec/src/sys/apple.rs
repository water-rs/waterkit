//! Apple `VideoToolbox` hardware encoding and decoding.
#![allow(clippy::duplicated_attributes)]

use objc2::rc::Retained;
use objc2_core_media::{
    CMSampleBuffer, CMSampleTimingInfo, CMTime, kCMVideoCodecType_H264, kCMVideoCodecType_HEVC,
};

use crate::CodecError;
use objc2_core_foundation::CFRetained;
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferLockBaseAddress,
    CVPixelBufferUnlockBaseAddress, kCVPixelBufferPixelFormatTypeKey,
    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
};
use objc2_io_surface::IOSurfaceRef;
use objc2_video_toolbox::{
    VTCompressionSession, VTDecodeInfoFlags, VTDecompressionOutputCallbackRecord, VTEncodeInfoFlags,
};
use std::ffi::c_void;
use std::fmt;
use std::ptr;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

/// Internal codec type for Apple implementations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecType {
    H264,
    H265,
}

#[link(name = "CoreMedia", kind = "framework")]
#[link(name = "VideoToolbox", kind = "framework")]
#[link(name = "CoreFoundation", kind = "framework")]
#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    static kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms: *const c_void;
    static kCFAllocatorDefault: *const c_void;
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;

    fn CMSampleBufferGetFormatDescription(sbuf: *mut CMSampleBuffer) -> *const c_void;
    fn CMFormatDescriptionGetExtension(desc: *const c_void, key: *const c_void) -> *const c_void;

    fn CFDictionaryGetValue(theDict: *const c_void, key: *const c_void) -> *const c_void;
    fn CFDataGetBytePtr(theData: *const c_void) -> *const u8;
    fn CFDataGetLength(theData: *const c_void) -> isize;

    fn CFStringCreateWithCString(
        alloc: *const c_void,
        cStr: *const i8,
        encoding: u32,
    ) -> *const c_void;
    fn CFRelease(cf: *const c_void);

    fn CMVideoFormatDescriptionCreate(
        allocator: *const c_void,
        codec_type: u32,
        width: i32,
        height: i32,
        extensions: *const c_void,                  // CFDictionaryRef
        format_description_out: *mut *const c_void, // CMVideoFormatDescriptionRef
    ) -> i32;

    fn CFDictionaryCreate(
        allocator: *const c_void,
        keys: *const *const c_void,
        values: *const *const c_void,
        numValues: isize,
        keyCallBacks: *const c_void,
        valueCallBacks: *const c_void,
    ) -> *const c_void; // CFDictionaryRef

    fn CFDataCreate(allocator: *const c_void, bytes: *const u8, length: isize) -> *const c_void; // CFDataRef

    fn VTDecompressionSessionCreate(
        allocator: *const c_void,
        format_description: *const c_void,
        decoder_specification: *const c_void,
        image_buffer_attributes: *const c_void,
        output_callback: *const VTDecompressionOutputCallbackRecord,
        decompression_session_out: *mut *mut c_void, // VTDecompressionSessionRef
    ) -> i32;

    fn VTDecompressionSessionDecodeFrame(
        session: *mut c_void,
        sample_buffer: *mut CMSampleBuffer,
        flags: u32,
        source_frame_ref_con: *mut c_void,
        info_flags_out: *mut u32,
    ) -> i32;

    fn VTDecompressionSessionWaitForAsynchronousFrames(session: *mut c_void) -> i32;
    fn VTDecompressionSessionInvalidate(session: *mut c_void);

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
        sample_buffer_out: *mut *mut CMSampleBuffer,
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
        blockBufferOut: *mut *const c_void, // CMBlockBufferRef
    ) -> i32;

    fn CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
        videoDesc: *const c_void,
        parameterSetIndex: usize,
        parameterSetPointerOut: *mut *const u8,
        parameterSetSizeOut: *mut usize,
        parameterSetCountOut: *mut usize,
        nalUnitHeaderLengthOut: *mut i32,
    ) -> i32;

    fn CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
        videoDesc: *const c_void,
        parameterSetIndex: usize,
        parameterSetPointerOut: *mut *const u8,
        parameterSetSizeOut: *mut usize,
        parameterSetCountOut: *mut usize,
        nalUnitHeaderLengthOut: *mut i32,
    ) -> i32;

    fn CVPixelBufferCreateWithIOSurface(
        allocator: *const c_void,
        surface: *const c_void, // IOSurfaceRef
        pixel_buffer_attributes: *const c_void,
        pixel_buffer_out: *mut *mut CVPixelBuffer,
    ) -> i32;

    fn CVPixelBufferGetIOSurface(pixelBuffer: *const CVPixelBuffer) -> *const IOSurfaceRef;

    fn CFNumberCreate(
        allocator: *const c_void,
        theType: i32,
        valuePtr: *const c_void,
    ) -> *const c_void;

    fn CMBlockBufferReplaceDataBytes(
        sourceBytes: *const c_void,
        destinationBuffer: *const c_void,
        offsetIntoDestination: usize,
        dataLength: usize,
    ) -> i32;
}

/// Apple `VideoToolbox` hardware encoder.
pub struct AppleEncoder {
    session: Retained<VTCompressionSession>,
    context: Arc<EncoderContext>,
    width: u32,
    height: u32,
    frame_count: i64,
}

impl fmt::Debug for AppleEncoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppleEncoder")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("frame_count", &self.frame_count)
            .finish_non_exhaustive()
    }
}

struct EncoderContext {
    encoded_data: Mutex<Vec<u8>>,
    codec_config: Mutex<Option<Vec<u8>>>,
}

// C-callback for VideoToolbox - receives encoded data
// C-callback for VideoToolbox - receives encoded data
#[allow(clippy::too_many_lines)]
#[allow(clippy::collapsible_if)]
unsafe extern "C-unwind" fn encode_callback(
    output_callback_ref_con: *mut c_void,
    _source_frame_ref_con: *mut c_void,
    status: i32,
    _info_flags: VTEncodeInfoFlags,
    sample_buffer: *mut CMSampleBuffer,
) {
    if status != 0 {
        eprintln!("VTCompressionSession callback error: {status}");
        return;
    }

    if sample_buffer.is_null() {
        return;
    }

    // Cast context
    let context = unsafe { &*(output_callback_ref_con as *const EncoderContext) };

    // Extract encoded data
    unsafe {
        // Use the method-based API: CMSampleBuffer::data_buffer()
        let sample_buf_ref = &*sample_buffer;
        if let Some(data_buffer) = sample_buf_ref.data_buffer() {
            let data_len = data_buffer.data_length();
            if data_len > 0 {
                let mut encoded_data = vec![0u8; data_len];
                let dest_ptr = NonNull::new(encoded_data.as_mut_ptr().cast::<c_void>()).unwrap();
                let result = data_buffer.copy_data_bytes(0, data_len, dest_ptr);

                if result == 0 {
                    if let Ok(mut lock) = context.encoded_data.lock() {
                        lock.extend_from_slice(&encoded_data);
                    }
                }
            }
        }

        // Extract codec config if needed
        let need_config = context.codec_config.lock().is_ok_and(|lock| lock.is_none());

        if need_config {
            fn construct_hevc_config(format_desc: *const c_void) -> Option<Vec<u8>> {
                unsafe {
                    // Get count first (index 0, pointers null)
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
                            &raw mut ptr,
                            &raw mut size,
                            ptr::null_mut(),
                            &raw mut header_len,
                        );

                        if status != 0 {
                            eprintln!(
                                "GetHEVCParameterSetAtIndex failed at index {index}: {status}"
                            );
                            break;
                        }

                        let data = std::slice::from_raw_parts(ptr, size).to_vec();
                        eprintln!(
                            "Found HEVC NAL at index {}: len={}, type={}",
                            index,
                            size,
                            (data[0] >> 1) & 0x3F
                        );

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
                    let num_arrays = u8::from(!vps_list.is_empty())
                        + u8::from(!sps_list.is_empty())
                        + u8::from(!pps_list.is_empty());
                    config.push(num_arrays);

                    // Arrays
                    let mut write_array = |nal_type: u8, list: &Vec<Vec<u8>>| {
                        if list.is_empty() {
                            return;
                        }
                        // Array header: completeness(1bit), reserved(1bit), nal_unit_type(6bits)
                        config.push(0x80 | (nal_type & 0x3F)); // completeness=1

                        // Num NALUs
                        let count = u16::try_from(list.len()).unwrap_or(0);
                        config.push((count >> 8) as u8);
                        config.push((count & 0xFF) as u8);

                        for nal in list {
                            let len = u16::try_from(nal.len()).unwrap_or(0);
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
                            &raw mut ptr,
                            &raw mut size,
                            ptr::null_mut(),
                            &raw mut header_len,
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
                    config.push(0xE0 | (u8::try_from(sps_list.len()).unwrap_or(0) & 0x1F));
                    for sps in &sps_list {
                        let len = u16::try_from(sps.len()).unwrap_or(0);
                        config.push((len >> 8) as u8);
                        config.push((len & 0xFF) as u8);
                        config.extend_from_slice(sps);
                    }

                    // Num PPS
                    config.push(u8::try_from(pps_list.len()).unwrap_or(0));
                    for pps in &pps_list {
                        let len = u16::try_from(pps.len()).unwrap_or(0);
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
                    // We should know codec type from somewhere, but here we can try both or check specific
                    // Ideally we check codec info.
                    // For now, let's focus on hvcC replacement logic.

                    let key_str = CFStringCreateWithCString(
                        kCFAllocatorDefault,
                        hvc_c_str.as_ptr().cast::<i8>(),
                        0x0800_0100,
                    );

                    if !key_str.is_null() {
                        let hvc_data = CFDictionaryGetValue(atoms, key_str);
                        if !hvc_data.is_null() {
                            let len = CFDataGetLength(hvc_data);
                            let ptr = CFDataGetBytePtr(hvc_data);
                            if len > 20 && !ptr.is_null() {
                                // Basic check: > 20 bytes for HEVC
                                let config_bytes =
                                    std::slice::from_raw_parts(ptr, len.cast_unsigned()).to_vec();
                                if let Ok(mut lock) = context.codec_config.lock() {
                                    eprintln!(
                                        "Found atomic hvcC extension with size {len}: {config_bytes:02X?}"
                                    );
                                    *lock = Some(config_bytes);
                                    found_config = true;
                                }
                            } else {
                                eprintln!("Ignored atomic hvcC extension with size {len}");
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
                        if let Some(config) = manual_avc
                            && let Ok(mut lock) = context.codec_config.lock()
                        {
                            *lock = Some(config);
                        }
                    }
                }
            }
        }
    }
}

impl AppleEncoder {
    /// Create a new Apple hardware encoder with default 1080p dimensions.
    #[allow(dead_code)]
    pub fn new(codec: CodecType) -> Result<Self, CodecError> {
        Self::with_size(codec, 1920, 1080)
    }

    /// Create encoder with specific dimensions.
    ///
    /// # Errors
    ///
    /// Returns `CodecError::InitializationFailed` if `VideoToolbox` session creation fails.
    ///
    /// # Panics
    ///
    /// Panics if the internal session pointer cannot be wrapped in `NonNull`.
    pub fn with_size(codec: CodecType, width: u32, height: u32) -> Result<Self, CodecError> {
        let codec_type = match codec {
            CodecType::H264 => kCMVideoCodecType_H264,
            CodecType::H265 => kCMVideoCodecType_HEVC,
        };

        let context = Arc::new(EncoderContext {
            encoded_data: Mutex::new(Vec::new()),
            codec_config: Mutex::new(None),
        });
        let context_ptr = Arc::as_ptr(&context) as *mut c_void;

        let mut session_ptr: *mut VTCompressionSession = ptr::null_mut();

        unsafe {
            let status = VTCompressionSession::create(
                None, // allocator
                width.cast_signed(),
                height.cast_signed(),
                codec_type,
                None, // encoderSpecification
                None, // sourceImageBufferAttributes
                None, // compressedDataAllocator
                Some(encode_callback),
                context_ptr,
                NonNull::new(&raw mut session_ptr).unwrap(),
            );

            if status != 0 {
                return Err(CodecError::InitializationFailed(format!(
                    "VT error: {status}"
                )));
            }
        }

        let session = unsafe { Retained::retain(session_ptr) }
            .ok_or_else(|| CodecError::InitializationFailed("Failed to retain session".into()))?;

        Ok(Self {
            session,
            context,
            width,
            height,
            frame_count: 0,
        })
    }

    /// Encode directly from `IOSurface` pointer (zero-copy from `ScreenCaptureKit`).
    ///
    /// This method takes an `IOSurface` pointer and creates a `CVPixelBuffer` from it,
    /// allowing `VideoToolbox` to encode directly from GPU memory without any CPU copy.
    ///
    /// # Errors
    ///
    /// Returns `CodecError::EncodingFailed` if `CVPixelBuffer` creation or encoding fails.
    pub fn encode_iosurface(&mut self, iosurface_ptr: u64) -> Result<Vec<u8>, CodecError> {
        if iosurface_ptr == 0 {
            return Err(CodecError::EncodingFailed("NULL IOSurface pointer".into()));
        }

        // Create CVPixelBuffer from IOSurface (zero-copy)
        let mut pixel_buffer_ptr: *mut CVPixelBuffer = ptr::null_mut();
        let pixel_buffer = unsafe {
            let status = CVPixelBufferCreateWithIOSurface(
                ptr::null(),                    // default allocator
                iosurface_ptr as *const c_void, // IOSurfaceRef
                ptr::null(),                    // pixelBufferAttributes
                &raw mut pixel_buffer_ptr,
            );

            if status != 0 || pixel_buffer_ptr.is_null() {
                return Err(CodecError::EncodingFailed(format!(
                    "CVPixelBufferCreateWithIOSurface failed: {status}"
                )));
            }

            // Follows CoreFoundation create rule: returned object is +1 retained.
            CFRetained::from_raw(NonNull::new_unchecked(pixel_buffer_ptr))
        };

        // Clear output buffer
        if let Ok(mut lock) = self.context.encoded_data.lock() {
            lock.clear();
        }

        // Encode the frame
        let pixel_buffer_ref: &CVPixelBuffer = &pixel_buffer;

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
                &raw mut info_flags,
            );

            if status != 0 {
                return Err(CodecError::EncodingFailed(format!(
                    "encode_frame failed: {status}"
                )));
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
                return Err(CodecError::EncodingFailed(format!(
                    "complete_frames failed: {complete_status}"
                )));
            }
        }

        // Return encoded data
        let result = self
            .context
            .encoded_data
            .lock()
            .map(|lock| lock.clone())
            .map_err(|_| CodecError::EncodingFailed("Lock error".into()))?;

        Ok(result)
    }

    /// Get the codec configuration data (e.g. hvcC or avcC atom) if available.
    #[must_use]
    pub fn get_codec_config(&self) -> Option<Vec<u8>> {
        self.context
            .codec_config
            .lock()
            .map_or(None, |lock| lock.clone())
    }
}

impl AppleEncoder {
    /// Encode NV12 data.
    #[allow(clippy::too_many_lines)]
    pub fn encode_nv12(&mut self, nv12: &[u8]) -> Result<Vec<u8>, CodecError> {
        let y_size = (self.width * self.height) as usize;
        let uv_size = y_size / 2;
        let expected_size = y_size + uv_size;

        if nv12.len() != expected_size {
            return Err(CodecError::EncodingFailed(format!(
                "NV12 data size {} doesn't match expected {} for {}x{}",
                nv12.len(),
                expected_size,
                self.width,
                self.height
            )));
        }

        // Create CVPixelBuffer with NV12 format
        let mut pixel_buffer_ptr: *mut CVPixelBuffer = ptr::null_mut();
        let pixel_buffer = unsafe {
            let status = CVPixelBufferCreate(
                None,
                self.width as usize,
                self.height as usize,
                kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                None,
                NonNull::new(&raw mut pixel_buffer_ptr).unwrap(),
            );

            if status != 0 || pixel_buffer_ptr.is_null() {
                return Err(CodecError::EncodingFailed(format!(
                    "CVPixelBufferCreate failed: {status}"
                )));
            }
            // Follows CoreFoundation create rule: returned object is +1 retained.
            CFRetained::from_raw(NonNull::new_unchecked(pixel_buffer_ptr))
        };

        let pixel_buffer_ref: &CVPixelBuffer = &pixel_buffer;

        // Copy NV12 data to pixel buffer planes
        unsafe {
            use objc2_core_video::{
                CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRowOfPlane,
                CVPixelBufferLockFlags,
            };

            let lock_status =
                CVPixelBufferLockBaseAddress(pixel_buffer_ref, CVPixelBufferLockFlags(0));
            if lock_status != 0 {
                return Err(CodecError::EncodingFailed(format!(
                    "CVPixelBufferLockBaseAddress failed: {lock_status}"
                )));
            }

            // Copy Y plane
            let y_base = CVPixelBufferGetBaseAddressOfPlane(pixel_buffer_ref, 0);
            let y_stride = CVPixelBufferGetBytesPerRowOfPlane(pixel_buffer_ref, 0);
            for row in 0..self.height as usize {
                ptr::copy_nonoverlapping(
                    nv12.as_ptr().add(row * self.width as usize),
                    y_base.cast::<u8>().add(row * y_stride),
                    self.width as usize,
                );
            }

            // Copy UV plane
            let uv_base = CVPixelBufferGetBaseAddressOfPlane(pixel_buffer_ref, 1);
            let uv_stride = CVPixelBufferGetBytesPerRowOfPlane(pixel_buffer_ref, 1);
            let uv_height = self.height as usize / 2;
            for row in 0..uv_height {
                ptr::copy_nonoverlapping(
                    nv12.as_ptr().add(y_size + row * self.width as usize),
                    uv_base.cast::<u8>().add(row * uv_stride),
                    self.width as usize,
                );
            }

            CVPixelBufferUnlockBaseAddress(pixel_buffer_ref, CVPixelBufferLockFlags(0));
        }

        // Clear output buffer
        if let Ok(mut lock) = self.context.encoded_data.lock() {
            lock.clear();
        }

        // Encode
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
                &raw mut info_flags,
            );

            if status != 0 {
                return Err(CodecError::EncodingFailed(format!(
                    "encode_frame failed: {status}"
                )));
            }

            let complete_time = CMTime {
                value: i64::MAX,
                timescale: 1,
                flags: CMTimeFlags(1),
                epoch: 0,
            };
            let complete_status = self.session.complete_frames(complete_time);

            if complete_status != 0 {
                return Err(CodecError::EncodingFailed(format!(
                    "complete_frames failed: {complete_status}"
                )));
            }
        }

        self.context
            .encoded_data
            .lock()
            .map(|lock| lock.clone())
            .map_err(|_| CodecError::EncodingFailed("Lock error".into()))
    }
}

struct DecoderContext {
    decoded_surfaces: Mutex<Vec<IOSurfaceFrame>>,
}

/// Apple `VideoToolbox` hardware decoder.
pub struct AppleDecoder {
    codec: CodecType,
    session: *mut c_void, // VTDecompressionSessionRef
    context: Arc<DecoderContext>,
    format_desc: *const c_void, // CMVideoFormatDescriptionRef
}

impl fmt::Debug for AppleDecoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppleDecoder")
            .field("codec", &self.codec)
            .finish_non_exhaustive()
    }
}

/// Zero-copy decoded frame backed by `IOSurface` (NV12 format).
#[derive(Clone)]
pub struct IOSurfaceFrame {
    /// The underlying `IOSurface`.
    pub surface: CFRetained<IOSurfaceRef>,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Presentation timestamp in nanoseconds.
    pub timestamp_ns: u64,
}

#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Send for IOSurfaceFrame {}

impl fmt::Debug for IOSurfaceFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IOSurfaceFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("timestamp_ns", &self.timestamp_ns)
            .finish_non_exhaustive()
    }
}

unsafe extern "C-unwind" fn decode_callback(
    decompression_output_ref_con: *mut c_void,
    _source_frame_ref_con: *mut c_void,
    status: i32,
    _info_flags: VTDecodeInfoFlags,
    image_buffer: *mut CVPixelBuffer,
    _presentation_time_stamp: CMTime,
    _presentation_duration: CMTime,
) {
    if status != 0 || image_buffer.is_null() {
        return;
    }

    let context = unsafe { &*(decompression_output_ref_con as *const DecoderContext) };
    let image_buffer_ref = unsafe { &*image_buffer };

    unsafe {
        let width =
            u32::try_from(objc2_core_video::CVPixelBufferGetWidth(image_buffer_ref)).unwrap_or(0);
        let height =
            u32::try_from(objc2_core_video::CVPixelBufferGetHeight(image_buffer_ref)).unwrap_or(0);

        let surface_raw = CVPixelBufferGetIOSurface(image_buffer_ref);
        if !surface_raw.is_null() {
            let surface = CFRetained::retain(NonNull::new_unchecked(surface_raw.cast_mut()));
            let frame = IOSurfaceFrame {
                surface,
                width,
                height,
                timestamp_ns: 0,
            };
            if let Ok(mut frames) = context.decoded_surfaces.lock() {
                frames.push(frame);
            }
        }
    }
}

impl AppleDecoder {
    /// Create a new Apple hardware decoder.
    #[allow(clippy::too_many_lines)]
    pub fn new(
        codec: CodecType,
        config: Option<&[u8]>,
        width: u32,
        height: u32,
    ) -> Result<Self, CodecError> {
        let Some(config_bytes) = config else {
            return Err(CodecError::InitializationFailed(
                "Codec config (hvcC/avcC) required".into(),
            ));
        };

        let codec_type = match codec {
            CodecType::H264 => kCMVideoCodecType_H264,
            CodecType::H265 => kCMVideoCodecType_HEVC,
        };

        let mut final_config = config_bytes;
        // Strip Box Header if present
        if final_config.len() > 8 {
            let atom_key = match codec {
                CodecType::H264 => b"avcC",
                CodecType::H265 => b"hvcC",
            };
            if &final_config[4..8] == atom_key {
                final_config = &final_config[8..];
            }
        }

        let context = Arc::new(DecoderContext {
            decoded_surfaces: Mutex::new(Vec::new()),
        });

        unsafe {
            let atom_key_str = if codec == CodecType::H265 {
                b"hvcC\0"
            } else {
                b"avcC\0"
            };
            let key_cf = CFStringCreateWithCString(
                kCFAllocatorDefault,
                atom_key_str.as_ptr().cast(),
                0x0800_0100,
            );

            let data_cf = CFDataCreate(
                kCFAllocatorDefault,
                final_config.as_ptr(),
                final_config.len().cast_signed(),
            );

            let keys = [key_cf];
            let values = [data_cf];
            let atoms_dict = CFDictionaryCreate(
                kCFAllocatorDefault,
                keys.as_ptr(),
                values.as_ptr(),
                1,
                &raw const kCFTypeDictionaryKeyCallBacks,
                &raw const kCFTypeDictionaryValueCallBacks,
            );

            let ext_key_str = b"SampleDescriptionExtensionAtoms\0";
            let ext_key_cf = CFStringCreateWithCString(
                kCFAllocatorDefault,
                ext_key_str.as_ptr().cast(),
                0x0800_0100,
            );

            let ext_keys = [ext_key_cf];
            let ext_values = [atoms_dict];
            let extensions = CFDictionaryCreate(
                kCFAllocatorDefault,
                ext_keys.as_ptr(),
                ext_values.as_ptr(),
                1,
                &raw const kCFTypeDictionaryKeyCallBacks,
                &raw const kCFTypeDictionaryValueCallBacks,
            );

            let mut format_desc: *const c_void = ptr::null();
            let status = CMVideoFormatDescriptionCreate(
                kCFAllocatorDefault,
                codec_type,
                width.cast_signed(),
                height.cast_signed(),
                extensions,
                &raw mut format_desc,
            );

            CFRelease(key_cf);
            CFRelease(data_cf);
            CFRelease(atoms_dict);
            CFRelease(ext_key_cf);
            CFRelease(extensions);

            if status != 0 {
                return Err(CodecError::InitializationFailed(format!(
                    "CMVideoFormatDescriptionCreate failed: {status}"
                )));
            }

            let callback_record = VTDecompressionOutputCallbackRecord {
                decompressionOutputCallback: Some(decode_callback),
                decompressionOutputRefCon: Arc::as_ptr(&context) as *mut c_void,
            };

            // Request NV12 output (420v)
            let pixel_format_nv12: u32 = 0x3432_3076; // '420v'
            let pixel_format_number = CFNumberCreate(
                kCFAllocatorDefault,
                3,
                ptr::from_ref(&pixel_format_nv12).cast::<c_void>(),
            );

            let pixel_format_key: *const c_void =
                ptr::from_ref(kCVPixelBufferPixelFormatTypeKey).cast::<c_void>();

            let attr_keys = [pixel_format_key];
            let attr_values = [pixel_format_number];
            let image_buffer_attrs = CFDictionaryCreate(
                kCFAllocatorDefault,
                attr_keys.as_ptr(),
                attr_values.as_ptr(),
                1,
                &raw const kCFTypeDictionaryKeyCallBacks,
                &raw const kCFTypeDictionaryValueCallBacks,
            );

            let mut session: *mut c_void = ptr::null_mut();
            let status = VTDecompressionSessionCreate(
                kCFAllocatorDefault,
                format_desc,
                ptr::null(),
                image_buffer_attrs,
                ptr::from_ref(&callback_record),
                &raw mut session,
            );

            CFRelease(pixel_format_number);
            CFRelease(image_buffer_attrs);

            if status != 0 {
                CFRelease(format_desc);
                return Err(CodecError::InitializationFailed(format!(
                    "VTDecompressionSessionCreate failed: {status}"
                )));
            }

            Ok(Self {
                codec,
                session,
                context,
                format_desc,
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
    /// Decode compressed video data to `IOSurface` frames.
    #[allow(clippy::too_many_lines)]
    pub fn decode_to_iosurface(&mut self, data: &[u8]) -> Result<Vec<IOSurfaceFrame>, CodecError> {
        if data.len() < 4 {
            return Err(CodecError::DecodingFailed("Data too short".into()));
        }

        unsafe {
            let mut block_buffer: *const c_void = ptr::null();
            let status = CMBlockBufferCreateWithMemoryBlock(
                kCFAllocatorDefault,
                ptr::null_mut(),
                data.len(),
                kCFAllocatorDefault,
                ptr::null(),
                0,
                data.len(),
                0,
                &raw mut block_buffer,
            );

            if status != 0 {
                return Err(CodecError::DecodingFailed(format!(
                    "CMBlockBufferCreate failed: {status}"
                )));
            }

            let status = CMBlockBufferReplaceDataBytes(
                data.as_ptr().cast::<c_void>(),
                block_buffer,
                0,
                data.len(),
            );

            if status != 0 {
                CFRelease(block_buffer);
                return Err(CodecError::DecodingFailed(format!(
                    "CMBlockBufferReplaceDataBytes failed: {status}"
                )));
            }

            let mut sample_buffer: *mut CMSampleBuffer = ptr::null_mut();
            let invalid_time = CMTime {
                value: 0,
                timescale: 0,
                flags: objc2_core_media::CMTimeFlags(0),
                epoch: 0,
            };

            let timing_info = CMSampleTimingInfo {
                duration: invalid_time,
                presentationTimeStamp: invalid_time,
                decodeTimeStamp: invalid_time,
            };

            let status = CMSampleBufferCreate(
                kCFAllocatorDefault,
                block_buffer,
                1,
                ptr::null(),
                ptr::null_mut(),
                self.format_desc,
                1,
                1,
                ptr::from_ref(&timing_info).cast::<c_void>(),
                0,
                ptr::null(),
                &raw mut sample_buffer,
            );

            CFRelease(block_buffer);

            if status != 0 {
                return Err(CodecError::DecodingFailed(format!(
                    "CMSampleBufferCreate failed: {status}"
                )));
            }

            let mut info_flags = 0;
            let status = VTDecompressionSessionDecodeFrame(
                self.session,
                sample_buffer,
                0,
                ptr::null_mut(),
                &raw mut info_flags,
            );

            CFRelease(sample_buffer.cast::<c_void>());

            if status != 0 {
                return Err(CodecError::DecodingFailed(format!(
                    "Decode failed: {status}"
                )));
            }

            let wait_status = VTDecompressionSessionWaitForAsynchronousFrames(self.session);
            if wait_status != 0 {
                return Err(CodecError::DecodingFailed(format!(
                    "Wait failed: {wait_status}"
                )));
            }
        }

        let mut frames = Vec::new();
        if let Ok(mut lock) = self.context.decoded_surfaces.lock() {
            frames.append(&mut lock);
        }
        Ok(frames)
    }
}

#[cfg(test)]
mod tests {
    use super::{AppleDecoder, CodecType};

    // Big Buck Bunny H.264 avcC from ffprobe.
    const AVC_CONFIG: &[u8] = &[
        0x01, 0x64, 0x00, 0x32, 0xFF, 0xE1, 0x00, 0x1B, 0x67, 0x64, 0x00, 0x32, 0xAC, 0x72, 0x84,
        0x40, 0x50, 0x05, 0xBB, 0x01, 0x10, 0x00, 0x00, 0x03, 0x00, 0x10, 0x00, 0x00, 0x03, 0x03,
        0xC0, 0xF1, 0x83, 0x18, 0x46, 0x01, 0x00, 0x07, 0x68, 0xE8, 0x43, 0x87, 0x4B, 0x22, 0xC0,
    ];

    #[test]
    fn decoder_init_h264_smoke() {
        let decoder = AppleDecoder::new(CodecType::H264, Some(AVC_CONFIG), 1280, 720);
        assert!(decoder.is_ok(), "decoder init failed: {decoder:?}");
    }
}
