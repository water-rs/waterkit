<<<<<<< HEAD
use half::f16;
use image::{ColorType, DynamicImage, GenericImageView};

use crate::CodecError;

/// Pixel formats currently emitted by `decode_image`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecodedPixelFormat {
    /// 8-bit normalized sRGB RGBA.
    Rgba8UnormSrgb,
    /// 16-bit float RGBA (reserved for HDR decoders).
    Rgba16Float,
}

/// Result of an image decode request.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DecodedImage {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    pixel_format: DecodedPixelFormat,
    hdr: bool,
    wide_gamut: bool,
}

impl DecodedImage {
    /// Create a new `DecodedImage`.
    pub(crate) const fn new(
        pixels: Vec<u8>,
        width: u32,
        height: u32,
        pixel_format: DecodedPixelFormat,
        hdr: bool,
        wide_gamut: bool,
    ) -> Self {
        Self {
            pixels,
            width,
            height,
            pixel_format,
            hdr,
            wide_gamut,
        }
    }

    /// Decoded image pixels in RGBA order.
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Consume this image and return the pixel data.
    #[must_use]
    pub fn into_pixels(self) -> Vec<u8> {
        self.pixels
    }

    /// Decoded image width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Decoded image height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Pixel format describing the pixel data.
    #[must_use]
    pub const fn pixel_format(&self) -> DecodedPixelFormat {
        self.pixel_format
    }

    /// Whether the decoded source is HDR.
    #[must_use]
    pub const fn hdr(&self) -> bool {
        self.hdr
    }

    /// Whether the decoded source uses wide gamut.
    #[must_use]
    pub const fn wide_gamut(&self) -> bool {
        self.wide_gamut
    }
}

/// Decodes image bytes into RGBA pixels.
///
/// # Errors
///
/// Returns [`CodecError::DecodingFailed`] when decoding fails.
pub fn decode_image(data: &[u8]) -> Result<DecodedImage, CodecError> {
    let decoded = image::load_from_memory(data)
        .map_err(|err| CodecError::DecodingFailed(format!("image decode failed: {err}")))?;
    let (width, height) = decoded.dimensions();
    let color = decoded.color();

    if is_high_precision_color(color) {
        // `image` does not reliably expose transfer/gamut metadata for every codec, so keep
        // HDR/wide-gamut flags conservative to avoid false positives.
        let (pixels, has_hdr_headroom) = encode_rgba16f(decoded, color);
        return Ok(DecodedImage::new(
            pixels,
            width,
            height,
            DecodedPixelFormat::Rgba16Float,
            has_hdr_headroom,
            false,
        ));
    }

    Ok(DecodedImage::new(
        decoded.into_rgba8().into_raw(),
        width,
        height,
        DecodedPixelFormat::Rgba8UnormSrgb,
        false,
        false,
    ))
}

const fn is_high_precision_color(color: ColorType) -> bool {
    matches!(color, ColorType::Rgb32F | ColorType::Rgba32F)
}

fn encode_rgba16f(image: DynamicImage, color: ColorType) -> (Vec<u8>, bool) {
    let mut output = Vec::new();
    let mut has_hdr_headroom = false;
    match color {
        ColorType::Rgb32F | ColorType::Rgba32F => {
            let rgba = image.into_rgba32f().into_raw();
            output.reserve(rgba.len() * core::mem::size_of::<u16>());
            for channel in rgba {
                has_hdr_headroom |= channel.is_finite() && channel > 1.0;
                output.extend_from_slice(&f16::from_f32(channel).to_le_bytes());
            }
        }
        _ => unreachable!("encode_rgba16f only supports 32-bit float inputs"),
    }
    (output, has_hdr_headroom)
=======
use crate::CodecError;

/// Pixel format returned by platform image decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodedPixelFormat {
    /// RGBA8 in sRGB transfer space.
    Rgba8UnormSrgb,
    /// RGBA16 half-float in linear extended-sRGB space.
    Rgba16Float,
}

/// Decoded image pixels.
#[derive(Debug, Clone)]
pub struct DecodedImage {
    /// Pixel buffer (row-major). Stride is tightly packed.
    pub pixels: Vec<u8>,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Pixel format of `pixels`.
    pub pixel_format: DecodedPixelFormat,
    /// Whether the decoded image should be treated as HDR content.
    ///
    /// `true` means the decoder returned high dynamic range pixels (`Rgba16Float`).
    pub hdr: bool,
    /// Whether decoded pixels preserve wide-gamut color information.
    ///
    /// This is independent from [`Self::hdr`]. Wide-gamut SDR images can set this to `true`
    /// while `hdr` stays `false`.
    pub wide_gamut: bool,
}

/// Decode an encoded still image into platform-native pixels.
///
/// On Apple/Android this routes to system decoders (hardware-accelerated when supported by the
/// platform). Other targets currently return [`CodecError::Unsupported`].
pub fn decode_image(data: &[u8]) -> Result<DecodedImage, CodecError> {
    decode_image_impl(data, true)
}

/// Decode an encoded still image into RGBA8 pixels via platform codecs.
///
/// This compatibility helper always requests SDR `RGBA8` output.
pub fn decode_image_rgba8(data: &[u8]) -> Result<DecodedImage, CodecError> {
    decode_image_impl(data, false)
}

fn decode_image_impl(data: &[u8], prefer_hdr: bool) -> Result<DecodedImage, CodecError> {
    if data.is_empty() {
        return Err(CodecError::DecodingFailed("image data is empty".into()));
    }

    #[cfg(target_os = "android")]
    {
        return decode_image_android(data, prefer_hdr);
    }

    #[cfg(target_vendor = "apple")]
    {
        return decode_image_apple(data, prefer_hdr);
    }

    #[allow(unreachable_code)]
    Err(CodecError::Unsupported(
        "platform image decoder unavailable on this target".into(),
    ))
}

#[cfg(target_os = "android")]
fn decode_image_android(data: &[u8], prefer_hdr: bool) -> Result<DecodedImage, CodecError> {
    use jni::JavaVM;
    use jni::objects::{JByteArray, JObject, JValue};

    let ctx = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }
        .map_err(|e| CodecError::InitializationFailed(format!("failed to acquire JavaVM: {e}")))?;
    let mut env = vm.attach_current_thread().map_err(|e| {
        CodecError::InitializationFailed(format!("failed to attach JNI thread: {e}"))
    })?;

    let input = env.byte_array_from_slice(data).map_err(|e| {
        CodecError::InitializationFailed(format!("failed to allocate Java byte[]: {e}"))
    })?;
    let input_obj = JObject::from(input);
    let prefer_hdr: jni::sys::jboolean = if prefer_hdr { 1 } else { 0 };

    let output_obj = env
        .call_static_method(
            "dev/waterui/android/runtime/ImageCodecBridge",
            "decodeToPackedImageV2",
            "([BZ)[B",
            &[JValue::Object(&input_obj), JValue::Bool(prefer_hdr)],
        )
        .map_err(|e| CodecError::DecodingFailed(format!("platform decode call failed: {e}")))?
        .l()
        .map_err(|e| {
            CodecError::DecodingFailed(format!("platform decode returned invalid value: {e}"))
        })?;

    if output_obj.is_null() {
        return Err(CodecError::DecodingFailed(
            "platform decoder returned null".into(),
        ));
    }

    let output_array = JByteArray::from(output_obj);
    let output = env
        .convert_byte_array(&output_array)
        .map_err(|e| CodecError::DecodingFailed(format!("failed reading decode payload: {e}")))?;
    parse_packed_image(&output)
}

#[cfg(target_vendor = "apple")]
fn decode_image_apple(data: &[u8], prefer_hdr: bool) -> Result<DecodedImage, CodecError> {
    use core::ffi::c_void;
    use core::ptr::{self, null};

    type CFAllocatorRef = *const c_void;
    type CFDataRef = *const c_void;
    type CFDictionaryRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFIndex = isize;
    type CGImageSourceRef = *const c_void;
    type CGImageRef = *const c_void;
    type CGColorSpaceRef = *const c_void;
    type CGContextRef = *mut c_void;

    #[repr(C)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    #[repr(C)]
    struct CGSize {
        width: f64,
        height: f64,
    }

    #[repr(C)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFDataCreate(allocator: CFAllocatorRef, bytes: *const u8, length: CFIndex) -> CFDataRef;
        fn CFRelease(cf: *const c_void);
    }

    #[link(name = "ImageIO", kind = "framework")]
    unsafe extern "C" {
        fn CGImageSourceCreateWithData(
            data: CFDataRef,
            options: CFDictionaryRef,
        ) -> CGImageSourceRef;
        fn CGImageSourceCreateImageAtIndex(
            source: CGImageSourceRef,
            index: usize,
            options: CFDictionaryRef,
        ) -> CGImageRef;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        static kCGColorSpaceSRGB: CFStringRef;
        static kCGColorSpaceExtendedLinearSRGB: CFStringRef;
        fn CGImageGetWidth(image: CGImageRef) -> usize;
        fn CGImageGetHeight(image: CGImageRef) -> usize;
        fn CGImageGetBitsPerComponent(image: CGImageRef) -> usize;
        fn CGImageGetColorSpace(image: CGImageRef) -> CGColorSpaceRef;
        fn CGColorSpaceIsWideGamutRGB(space: CGColorSpaceRef) -> bool;
        fn CGImageGetAlphaInfo(image: CGImageRef) -> u32;
        fn CGColorSpaceCreateWithName(name: CFStringRef) -> CGColorSpaceRef;
        fn CGBitmapContextCreate(
            data: *mut c_void,
            width: usize,
            height: usize,
            bits_per_component: usize,
            bytes_per_row: usize,
            space: CGColorSpaceRef,
            bitmap_info: u32,
        ) -> CGContextRef;
        fn CGContextDrawImage(context: CGContextRef, rect: CGRect, image: CGImageRef);
        fn CGBitmapContextGetData(context: CGContextRef) -> *mut c_void;
    }

    struct AppleCfOwned(*const c_void);

    impl Drop for AppleCfOwned {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0) };
            }
        }
    }

    const K_CG_IMAGE_ALPHA_PREMULTIPLIED_LAST: u32 = 1;
    const K_CG_IMAGE_ALPHA_NONE: u32 = 0;
    const K_CG_IMAGE_ALPHA_NONE_SKIP_LAST: u32 = 5;
    const K_CG_IMAGE_ALPHA_NONE_SKIP_FIRST: u32 = 6;
    const K_CG_BITMAP_FLOAT_COMPONENTS: u32 = 1 << 8;
    const K_CG_BITMAP_BYTE_ORDER_16_LITTLE: u32 = 1 << 12;
    const K_CG_BITMAP_BYTE_ORDER_32_BIG: u32 = 4 << 12;
    const BITMAP_INFO_RGBA8_PREMULTIPLIED_LAST_BIG_ENDIAN: u32 =
        K_CG_IMAGE_ALPHA_PREMULTIPLIED_LAST | K_CG_BITMAP_BYTE_ORDER_32_BIG;
    const BITMAP_INFO_RGBA8_NONE_SKIP_LAST_BIG_ENDIAN: u32 =
        K_CG_IMAGE_ALPHA_NONE_SKIP_LAST | K_CG_BITMAP_BYTE_ORDER_32_BIG;
    const BITMAP_INFO_RGBA16F_PREMULTIPLIED_LAST_LITTLE_ENDIAN: u32 =
        K_CG_IMAGE_ALPHA_PREMULTIPLIED_LAST
            | K_CG_BITMAP_FLOAT_COMPONENTS
            | K_CG_BITMAP_BYTE_ORDER_16_LITTLE;
    const BITMAP_INFO_RGBA16F_NONE_SKIP_LAST_LITTLE_ENDIAN: u32 = K_CG_IMAGE_ALPHA_NONE_SKIP_LAST
        | K_CG_BITMAP_FLOAT_COMPONENTS
        | K_CG_BITMAP_BYTE_ORDER_16_LITTLE;

    let cf_data = unsafe { CFDataCreate(null(), data.as_ptr(), data.len() as CFIndex) };
    if cf_data.is_null() {
        return Err(CodecError::DecodingFailed(
            "failed to allocate CFData for image bytes".into(),
        ));
    }
    let _cf_data_guard = AppleCfOwned(cf_data);

    let source = unsafe { CGImageSourceCreateWithData(cf_data, null()) };
    if source.is_null() {
        return Err(CodecError::DecodingFailed(
            "ImageIO failed to parse image source".into(),
        ));
    }
    let _source_guard = AppleCfOwned(source);

    let image = unsafe { CGImageSourceCreateImageAtIndex(source, 0, null()) };
    if image.is_null() {
        return Err(CodecError::DecodingFailed(
            "ImageIO failed to decode image frame at index 0".into(),
        ));
    }
    let _image_guard = AppleCfOwned(image);

    let width = unsafe { CGImageGetWidth(image) };
    let height = unsafe { CGImageGetHeight(image) };
    if width == 0 || height == 0 {
        return Err(CodecError::DecodingFailed(
            "decoded image has invalid zero dimensions".into(),
        ));
    }

    let source_bpc = unsafe { CGImageGetBitsPerComponent(image) };
    let source_color_space = unsafe { CGImageGetColorSpace(image) };
    let source_is_wide_gamut =
        !source_color_space.is_null() && unsafe { CGColorSpaceIsWideGamutRGB(source_color_space) };
    let alpha_info = unsafe { CGImageGetAlphaInfo(image) };
    let source_has_alpha = !matches!(
        alpha_info,
        K_CG_IMAGE_ALPHA_NONE | K_CG_IMAGE_ALPHA_NONE_SKIP_LAST | K_CG_IMAGE_ALPHA_NONE_SKIP_FIRST
    );
    let use_float = prefer_hdr && (source_bpc > 8 || source_is_wide_gamut);
    let (
        pixel_format,
        color_space_name,
        bits_per_component,
        bytes_per_pixel,
        bitmap_info,
        hdr,
        wide_gamut,
    ) = unsafe {
        if use_float {
            (
                DecodedPixelFormat::Rgba16Float,
                kCGColorSpaceExtendedLinearSRGB,
                16usize,
                8usize,
                if source_has_alpha {
                    BITMAP_INFO_RGBA16F_PREMULTIPLIED_LAST_LITTLE_ENDIAN
                } else {
                    BITMAP_INFO_RGBA16F_NONE_SKIP_LAST_LITTLE_ENDIAN
                },
                source_bpc > 8,
                source_is_wide_gamut,
            )
        } else {
            (
                DecodedPixelFormat::Rgba8UnormSrgb,
                kCGColorSpaceSRGB,
                8usize,
                4usize,
                if source_has_alpha {
                    BITMAP_INFO_RGBA8_PREMULTIPLIED_LAST_BIG_ENDIAN
                } else {
                    BITMAP_INFO_RGBA8_NONE_SKIP_LAST_BIG_ENDIAN
                },
                false,
                false,
            )
        }
    };

    let color_space = unsafe { CGColorSpaceCreateWithName(color_space_name) };
    if color_space.is_null() {
        return Err(CodecError::DecodingFailed(
            "CoreGraphics failed to create decode color space".into(),
        ));
    }
    let _color_space_guard = AppleCfOwned(color_space);

    let bytes_per_row = width
        .checked_mul(bytes_per_pixel)
        .ok_or_else(|| CodecError::DecodingFailed("image row size overflow".into()))?;
    let total_bytes = bytes_per_row
        .checked_mul(height)
        .ok_or_else(|| CodecError::DecodingFailed("image buffer size overflow".into()))?;

    let context = unsafe {
        CGBitmapContextCreate(
            ptr::null_mut(),
            width,
            height,
            bits_per_component,
            bytes_per_row,
            color_space,
            bitmap_info,
        )
    };
    if context.is_null() {
        return Err(CodecError::DecodingFailed(
            "CoreGraphics failed to create bitmap context".into(),
        ));
    }
    let _context_guard = AppleCfOwned(context.cast_const());

    let draw_rect = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width: width as f64,
            height: height as f64,
        },
    };
    unsafe { CGContextDrawImage(context, draw_rect, image) };

    let raw_pixels = unsafe { CGBitmapContextGetData(context) };
    if raw_pixels.is_null() {
        return Err(CodecError::DecodingFailed(
            "CoreGraphics bitmap context returned null pixel data".into(),
        ));
    }

    let mut pixels = unsafe {
        let slice = core::slice::from_raw_parts(raw_pixels.cast::<u8>(), total_bytes);
        slice.to_vec()
    };
    if !source_has_alpha {
        force_opaque_alpha(&mut pixels, pixel_format);
    }

    let width = u32::try_from(width)
        .map_err(|_| CodecError::DecodingFailed("image width exceeds u32".into()))?;
    let height = u32::try_from(height)
        .map_err(|_| CodecError::DecodingFailed("image height exceeds u32".into()))?;

    validate_dimensions(width, height, pixel_format, pixels.len())?;
    Ok(DecodedImage {
        pixels,
        width,
        height,
        pixel_format,
        hdr,
        wide_gamut,
    })
}

fn force_opaque_alpha(pixels: &mut [u8], pixel_format: DecodedPixelFormat) {
    match pixel_format {
        DecodedPixelFormat::Rgba8UnormSrgb => {
            for px in pixels.chunks_exact_mut(4) {
                px[3] = u8::MAX;
            }
        }
        DecodedPixelFormat::Rgba16Float => {
            // IEEE 754 half-float 1.0 = 0x3C00 (little-endian bytes: 00 3C).
            for px in pixels.chunks_exact_mut(8) {
                px[6] = 0x00;
                px[7] = 0x3C;
            }
        }
    }
}

#[cfg(target_os = "android")]
fn parse_packed_image(payload: &[u8]) -> Result<DecodedImage, CodecError> {
    const HEADER_LEN: usize = 12;
    if payload.len() < HEADER_LEN {
        return Err(CodecError::DecodingFailed(
            "platform decode payload too short".into(),
        ));
    }
    let width = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let height = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
    let pixel_format = match payload[8] {
        0 => DecodedPixelFormat::Rgba8UnormSrgb,
        1 => DecodedPixelFormat::Rgba16Float,
        other => {
            return Err(CodecError::DecodingFailed(format!(
                "invalid packed image pixel format tag: {other}"
            )));
        }
    };
    let hdr = payload[9] != 0;
    let wide_gamut = payload[10] != 0;
    let pixels = payload[HEADER_LEN..].to_vec();
    validate_dimensions(width, height, pixel_format, pixels.len())?;
    Ok(DecodedImage {
        pixels,
        width,
        height,
        pixel_format,
        hdr,
        wide_gamut,
    })
}

fn validate_dimensions(
    width: u32,
    height: u32,
    pixel_format: DecodedPixelFormat,
    pixel_len: usize,
) -> Result<(), CodecError> {
    let bytes_per_pixel = match pixel_format {
        DecodedPixelFormat::Rgba8UnormSrgb => 4usize,
        DecodedPixelFormat::Rgba16Float => 8usize,
    };
    let expected = width as usize * height as usize * bytes_per_pixel;
    if pixel_len != expected {
        return Err(CodecError::DecodingFailed(format!(
            "invalid pixel size: expected {expected} bytes for {width}x{height} ({pixel_format:?}), got {pixel_len}"
        )));
    }
    Ok(())
>>>>>>> main
}
