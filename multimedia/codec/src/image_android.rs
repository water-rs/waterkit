//! Android still-image decode through `android.graphics.BitmapFactory`.
//!
//! The NDK `AImageDecoder` needs API 30 while the framework floor is API 26,
//! so AVIF and HEIF stills go through the framework decoder. `BitmapFactory`
//! covers AVIF on Android 12 (API 31)+ and returns null for both unsupported
//! codecs and older releases — the failure surfaces as
//! [`CodecError::DecodingFailed`], never as a silent software fallback.

use jni::objects::{JObject, JValue, JValueOwned};
use jni::{Env, jni_sig, jni_str};
use ndk::bitmap::{Bitmap, BitmapFormat, BitmapInfoFlagsAlpha};

use crate::CodecError;
use crate::image::{DecodedImage, DecodedPixelFormat};

/// Decode an ISOBMFF still image (AVIF, HEIF) through the platform decoder.
///
/// # Errors
///
/// Returns [`CodecError::DecodingFailed`] when the platform cannot decode the
/// image — notably AVIF on Android 11 (API 30) and older.
pub fn decode_isobmff_android(data: &[u8]) -> Result<DecodedImage, CodecError> {
    let (vm, _context) = waterkit_build::jvm_and_context()
        .map_err(|err| CodecError::DecodingFailed(format!("JNI runtime unavailable: {err}")))?;

    vm.attach_current_thread(
        |env| -> Result<Result<DecodedImage, CodecError>, jni::errors::Error> {
            Ok(decode_attached(env, data))
        },
    )
    .map_err(|err| CodecError::DecodingFailed(format!("JNI attach error: {err}")))?
}

fn jni_failed(what: &'static str) -> impl Fn(jni::errors::Error) -> CodecError {
    move |err| CodecError::DecodingFailed(format!("{what}: {err}"))
}

/// A `BitmapFactory.Options` pinned to non-premultiplied `ARGB_8888`.
fn decode_options<'local>(
    env: &mut Env<'local>,
    argb_8888: &JObject<'local>,
) -> Result<JObject<'local>, CodecError> {
    let options = env
        .new_object(
            jni_str!("android/graphics/BitmapFactory$Options"),
            jni_sig!("()V"),
            &[],
        )
        .map_err(jni_failed("BitmapFactory.Options"))?;
    env.set_field(
        &options,
        jni_str!("inPremultiplied"),
        jni_sig!("Z"),
        JValue::Bool(false),
    )
    .map_err(jni_failed("Options.inPremultiplied"))?;
    env.set_field(
        &options,
        jni_str!("inPreferredConfig"),
        jni_sig!("Landroid/graphics/Bitmap$Config;"),
        JValue::Object(argb_8888),
    )
    .map_err(jni_failed("Options.inPreferredConfig"))?;
    Ok(options)
}

/// `BitmapFactory.decodeByteArray` with the `ARGB_8888` options.
fn decode_bitmap<'local>(
    env: &mut Env<'local>,
    data: &[u8],
    options: &JObject<'local>,
) -> Result<JObject<'local>, CodecError> {
    let bytes = env
        .byte_array_from_slice(data)
        .map_err(jni_failed("byte_array_from_slice"))?;
    env.call_static_method(
        jni_str!("android/graphics/BitmapFactory"),
        jni_str!("decodeByteArray"),
        jni_sig!("([BIILandroid/graphics/BitmapFactory$Options;)Landroid/graphics/Bitmap;"),
        &[
            JValue::Object(&bytes),
            JValue::Int(0),
            JValue::Int(
                i32::try_from(data.len()).map_err(|_| {
                    CodecError::DecodingFailed("image length exceeds i32::MAX".into())
                })?,
            ),
            JValue::Object(options),
        ],
    )
    .and_then(JValueOwned::l)
    .map_err(jni_failed("BitmapFactory.decodeByteArray"))
}

/// `android.os.Build$VERSION.SDK_INT`.
fn sdk_int(env: &mut Env<'_>) -> Result<i32, CodecError> {
    env.get_static_field(
        jni_str!("android/os/Build$VERSION"),
        jni_str!("SDK_INT"),
        jni_sig!("I"),
    )
    .and_then(JValueOwned::i)
    .map_err(jni_failed("Build$VERSION.SDK_INT"))
}

/// Unlocks the bitmap's pixels when dropped, so the copy can fail without
/// leaking the lock.
struct PixelsLock<'a> {
    bitmap: &'a Bitmap,
}

impl Drop for PixelsLock<'_> {
    fn drop(&mut self) {
        let _ = self.bitmap.unlock_pixels();
    }
}

/// Copy the bitmap's pixels into a tightly packed RGBA buffer through
/// `AndroidBitmap_lockPixels`.
///
/// Requires the bitmap to be `RGBA_8888` (enforced via `inPreferredConfig`
/// plus a [`BitmapInfo`](ndk::bitmap::BitmapInfo) format check) and, on API
/// 30+, unpremultiplied or opaque — `AndroidBitmapInfo.flags` is only
/// populated on API 30+, so the alpha check is skipped below that.
fn copy_pixels(bitmap: &Bitmap, sdk: i32) -> Result<(Vec<u8>, u32, u32), CodecError> {
    let info = bitmap
        .info()
        .map_err(|err| CodecError::DecodingFailed(format!("AndroidBitmap_getInfo: {err}")))?;
    let format = info.format();
    if format != BitmapFormat::RGBA_8888 {
        return Err(CodecError::DecodingFailed(format!(
            "BitmapFactory returned a non-RGBA_8888 bitmap: {format:?}"
        )));
    }
    if sdk >= 30 && matches!(info.flags().alpha(), BitmapInfoFlagsAlpha::Premultiplied) {
        return Err(CodecError::DecodingFailed(
            "BitmapFactory returned premultiplied pixels despite inPremultiplied=false".into(),
        ));
    }
    let (width, height) = (info.width(), info.height());
    let stride = info.stride() as usize;
    let row_len = (width as usize)
        .checked_mul(4)
        .ok_or_else(|| CodecError::DecodingFailed("bitmap width*4 overflows usize".into()))?;
    if row_len > stride {
        return Err(CodecError::DecodingFailed(format!(
            "bitmap stride {stride} smaller than width*4 {row_len}"
        )));
    }
    let out_len = row_len
        .checked_mul(height as usize)
        .ok_or_else(|| CodecError::DecodingFailed("bitmap size overflows usize".into()))?;

    let ptr = bitmap
        .lock_pixels()
        .map_err(|err| CodecError::DecodingFailed(format!("AndroidBitmap_lockPixels: {err}")))?;
    let _lock = PixelsLock { bitmap };
    let mut pixels = vec![0u8; out_len];
    for row in 0..height as usize {
        // Safety: `ptr` is valid for `stride * height` bytes while locked and
        // `row_len <= stride`, so each row read is in bounds.
        let src = unsafe { ptr.cast::<u8>().add(row * stride) };
        unsafe {
            src.copy_to_nonoverlapping(
                pixels[row * row_len..(row + 1) * row_len].as_mut_ptr(),
                row_len,
            );
        }
    }
    Ok((pixels, width, height))
}

fn decode_attached(env: &mut Env<'_>, data: &[u8]) -> Result<DecodedImage, CodecError> {
    let argb_8888 = env
        .get_static_field(
            jni_str!("android/graphics/Bitmap$Config"),
            jni_str!("ARGB_8888"),
            jni_sig!("Landroid/graphics/Bitmap$Config;"),
        )
        .and_then(JValueOwned::l)
        .map_err(jni_failed("Bitmap$Config.ARGB_8888"))?;

    let options = decode_options(env, &argb_8888)?;
    let bitmap = decode_bitmap(env, data, &options)?;

    if bitmap.is_null() {
        return Err(CodecError::DecodingFailed(
            "BitmapFactory could not decode this image; AVIF needs Android 12 (API 31)".into(),
        ));
    }

    // jni 0.22 and ndk 0.9 depend on different `jni-sys` versions, so the raw
    // pointers are cast across; both are opaque JNI handle types.
    let ndk_bitmap = unsafe {
        // Safety: `bitmap` is a live local reference to an android.graphics.Bitmap
        // and `env` is an attached JNI env for this thread.
        Bitmap::from_jni(env.get_raw().cast(), bitmap.as_raw().cast())
    };
    let (pixels, width, height) = copy_pixels(&ndk_bitmap, sdk_int(env)?)?;

    Ok(DecodedImage::new(
        pixels,
        width,
        height,
        DecodedPixelFormat::Rgba8UnormSrgb,
        false,
        false,
    ))
}
