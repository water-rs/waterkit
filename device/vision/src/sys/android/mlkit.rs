//! Shared plumbing for the `barcode` and `text` requests' native
//! realization: Play services ML Kit, unbundled.
//!
//! The `play-services-mlkit-*` artifacts are thin clients: the detection
//! engines live in modules Play services delivers on demand, never in the
//! app. `MlKitInput.kt` builds the `InputImage` both helpers take;
//! `VisionBarcodeHelper.kt`/`VisionTextHelper.kt` wrap the client calls and
//! each module's `ModuleInstallClient` install.
//!
//! Camera frames reach ML Kit without a CPU copy: the analysis stream's
//! `android.media.Image` goes straight into `InputImage.fromMediaImage`.
//! Encoded bytes decode through `BitmapFactory` with EXIF applied. Only a
//! `wgpu` texture reads back RGBA into a bitmap, as no public Android API
//! accepts a GPU texture.

use std::sync::Arc;

use futures::channel::oneshot;
use jni::objects::{Global, JClass, JObject, JValue, JValueOwned};
use jni::{Env, jni_sig, jni_str};
use waterkit_build::{
    AndroidError, DexHelper, FromJava, NativeCallback, describe_jni_error, dex_helper,
    with_android_context,
};

use crate::{
    Orientation, VisionError,
    geometry::{Point, Quad},
    image::Pixels,
    sealed::{Context, Preparation},
};

/// `waterkit.vision.MlKitInput`, compiled from this crate's
/// `kotlin-sources` and resolved through the application's class loader.
/// The `InputImage` builders both features share live here.
pub static INPUT: DexHelper = dex_helper!("waterkit.vision.MlKitInput");

/// `waterkit.vision.VisionBarcodeHelper`, holding the barcode scanner
/// clients.
#[cfg(feature = "barcode")]
pub static BARCODE_HELPER: DexHelper = dex_helper!("waterkit.vision.VisionBarcodeHelper");

/// `waterkit.vision.VisionTextHelper`, holding the script recognizer
/// clients.
#[cfg(feature = "text")]
pub static TEXT_HELPER: DexHelper = dex_helper!("waterkit.vision.VisionTextHelper");

// Module codes mirrored in `VisionTextHelper`: the recognizer `prepareModule`
// installs. `BarcodeHelper.prepareModule` takes no code — it has one module.
#[cfg(feature = "text")]
pub const MODULE_LATIN: i32 = 0;
#[cfg(feature = "text")]
pub const MODULE_CHINESE: i32 = 1;
#[cfg(feature = "text")]
pub const MODULE_DEVANAGARI: i32 = 2;
#[cfg(feature = "text")]
pub const MODULE_JAPANESE: i32 = 3;
#[cfg(feature = "text")]
pub const MODULE_KOREAN: i32 = 4;

/// Runs `work` with the Android context on a dedicated thread, so the JNI
/// calls and the bitmap decode/readback never block the awaiting task.
pub async fn on_vision_thread<T, F>(label: &'static str, work: F) -> Result<T, VisionError>
where
    T: Send + 'static,
    F: FnOnce(&mut Env<'_>, &JObject<'_>) -> Result<T, VisionError> + Send + 'static,
{
    let (sender, receiver) = oneshot::channel();
    std::thread::Builder::new()
        .name(String::from(label))
        .spawn(move || {
            let _ = sender.send(with_android_context(work));
        })
        .map_err(|error| VisionError::Platform(format!("spawn {label} thread: {error}")))?;
    receiver
        .await
        .map_err(|_| VisionError::Platform(format!("{label} thread died")))?
}

/// The `prepareModule` callback's payload: `null`, the helper's signal
/// that the module is ready to serve requests.
struct ModuleReady;

impl FromJava for ModuleReady {
    fn from_java(_env: &mut Env<'_>, object: &JObject<'_>) -> Result<Self, AndroidError> {
        if object.is_null() {
            Ok(Self)
        } else {
            Err(AndroidError::from(jni::errors::Error::WrongObjectType))
        }
    }
}

/// Installs `helper`'s module `module` — the barcode engine or a script's
/// recognizer — when Play services does not already have it.
///
/// The install answers through a `NativeCallback` the helper completes
/// from the tasks' listeners, so this resolves when Play services answers
/// and nothing is parked waiting.
///
/// # Errors
///
/// Returns [`VisionError::ModelUnavailable`] when the module install fails.
pub async fn prepare_module(helper: &DexHelper, module: i32) -> Result<(), VisionError> {
    let rx = with_android_context(|env, context| -> Result<_, VisionError> {
        let class = helper.class(env, context)?;
        let (callback, rx) = NativeCallback::<ModuleReady>::new(env)?;
        env.call_static_method(
            class,
            jni_str!("prepareModule"),
            jni_sig!("(Landroid/content/Context;Lwaterkit/build/NativeCallback;I)V"),
            &[
                JValue::Object(context),
                JValue::Object(callback.as_obj()),
                JValue::Int(module),
            ],
        )
        .map_err(|error| {
            VisionError::ModelUnavailable(format!(
                "module {module} install: {}",
                describe_jni_error(env, error)
            ))
        })?;
        Ok(rx)
    })?;
    rx.await
        .map_err(|_| {
            VisionError::ModelUnavailable(String::from(
                "the module-install callback was collected unanswered",
            ))
        })?
        .map_err(|error| VisionError::ModelUnavailable(error.to_string()))?;
    Ok(())
}

/// The `InputImage` every request in a pass shares, built once from the
/// pass' pixels.
#[derive(Debug)]
pub struct MlInput {
    /// The `InputImage` this pass' requests all read.
    pub input: Global<JObject<'static>>,
    /// The clockwise degrees detections are reported against; the points
    /// arrive in the stored orientation, and this rotation maps them to
    /// upright space.
    pub rotation_degrees: i32,
    /// Detection-space dimensions, matching `rotation_degrees`.
    pub width: u32,
    pub height: u32,
}

/// `Pass`-shared [`MlInput`], Arc'd so each request's worker thread takes
/// its own handle.
#[derive(Debug)]
pub struct SharedInput(pub Arc<MlInput>);

/// The clockwise rotation ML Kit must be told about, so detections come back
/// against the upright image. A mirrored orientation has no rotationDegrees
/// representation; those inputs cannot be served.
#[cfg(feature = "camera")]
fn rotation_degrees(orientation: Orientation) -> Result<i32, VisionError> {
    match orientation {
        Orientation::Up => Ok(0),
        Orientation::Right => Ok(90),
        Orientation::Down => Ok(180),
        Orientation::Left => Ok(270),
        mirrored => Err(VisionError::Unsupported(format!(
            "mirrored orientation {mirrored:?} cannot feed ML Kit"
        ))),
    }
}

/// Reads `texture` back into tightly packed RGBA bytes.
///
/// `Pixels::Texture` is the only CPU-copied still input path: ML Kit accepts
/// `android.media.Image`, `Bitmap`, bytes or a file, and a wgpu texture is
/// none of those.
fn readback_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
) -> Result<(Vec<u8>, u32, u32), VisionError> {
    if !texture.usage().contains(wgpu::TextureUsages::COPY_SRC) {
        return Err(VisionError::Unsupported(String::from(
            "the image texture lacks COPY_SRC, so its pixels cannot reach ML Kit",
        )));
    }
    let swizzle_bgra = match texture.format() {
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => false,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => true,
        format => {
            return Err(VisionError::Unsupported(format!(
                "ML Kit inputs need an 8-bit RGBA texture, not {format:?}"
            )));
        }
    };
    let size = texture.size();
    let row_bytes = size.width * 4;
    let padded_row = row_bytes.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("waterkit-vision texture readback"),
        size: u64::from(padded_row * size.height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row),
                rows_per_image: Some(size.height),
            },
        },
        size,
    );
    queue.submit([encoder.finish()]);
    let (sender, receiver) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(30)),
        })
        .map_err(|error| VisionError::Gpu(format!("texture readback poll: {error}")))?;
    receiver
        .recv()
        .map_err(|error| VisionError::Gpu(format!("texture readback map: {error}")))?
        .map_err(|error| VisionError::Gpu(format!("texture readback map: {error}")))?;
    let mapped = buffer
        .slice(..)
        .get_mapped_range()
        .map_err(|error| VisionError::Gpu(format!("texture readback range: {error}")))?;
    let mut rgba: Vec<u8> = mapped
        .chunks(padded_row as usize)
        .flat_map(|row| &row[..row_bytes as usize])
        .copied()
        .collect();
    drop(mapped);
    buffer.unmap();
    if swizzle_bgra {
        rgba.as_chunks_mut::<4>().0.iter_mut().for_each(|pixel| {
            pixel.swap(0, 2);
        });
    }
    Ok((rgba, size.width, size.height))
}

/// Builds the pass' `InputImage` from `pixels` on the worker thread.
fn input_image(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    pixels: &Pixels,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> Result<MlInput, VisionError> {
    let class = INPUT.class(env, context)?;
    let input = match pixels {
        Pixels::Encoded(bytes) => {
            let data = env
                .byte_array_from_slice(bytes)
                .map_err(|error| VisionError::Platform(format!("encode image bytes: {error}")))?;
            let bitmap = env
                .call_static_method(
                    class,
                    jni_str!("decode"),
                    jni_sig!("([B)Landroid/graphics/Bitmap;"),
                    &[JValue::Object(&data)],
                )
                .and_then(JValueOwned::l)
                .map_err(|error| {
                    VisionError::Platform(format!(
                        "decode image: {}",
                        describe_jni_error(env, error)
                    ))
                })?;
            // `decode` applies EXIF orientation, so the bitmap is upright.
            bitmap_input(env, class, &bitmap)?
        }
        Pixels::Texture {
            texture,
            orientation,
        } => {
            let (rgba, width, height) = readback_rgba(device, queue, texture)?;
            rgba_input(env, class, &rgba, width, height, *orientation)?
        }
        #[cfg(feature = "camera")]
        Pixels::Frame { .. } => {
            return Err(VisionError::Unsupported(String::from(
                "camera frames are GPU-only on Android and cannot feed ML \
                 Kit; open the camera with `CameraConfig::analysis` and serve \
                 the request from its `AnalysisFrame`",
            )));
        }
        #[cfg(feature = "camera")]
        Pixels::Analysis { frame } => {
            // `fromMediaImage` borrows the `android.media.Image` the
            // `AnalysisFrame` keeps acquired — no pixel copy.
            let rotation = rotation_degrees(frame.orientation())?;
            let image = env
                .call_static_method(
                    class,
                    jni_str!("mediaInput"),
                    jni_sig!("(Landroid/media/Image;I)Lcom/google/mlkit/vision/common/InputImage;"),
                    &[
                        JValue::Object(frame.media_image().as_obj()),
                        JValue::Int(rotation),
                    ],
                )
                .and_then(JValueOwned::l)
                .map_err(|error| {
                    VisionError::Platform(format!(
                        "media image input: {}",
                        describe_jni_error(env, error)
                    ))
                })?;
            (image, rotation)
        }
    };
    finish_input(env, input)
}

/// Wraps `bitmap` — already upright — as an `InputImage`.
fn bitmap_input<'local>(
    env: &mut Env<'local>,
    class: &Global<JClass<'static>>,
    bitmap: &JObject<'local>,
) -> Result<(JObject<'local>, i32), VisionError> {
    let input = env
        .call_static_method(
            class,
            jni_str!("bitmapInput"),
            jni_sig!("(Landroid/graphics/Bitmap;)Lcom/google/mlkit/vision/common/InputImage;"),
            &[JValue::Object(bitmap)],
        )
        .and_then(JValueOwned::l)
        .map_err(|error| {
            VisionError::Platform(format!("bitmap input: {}", describe_jni_error(env, error)))
        })?;
    Ok((input, 0))
}

/// Wraps tightly packed RGBA pixels as an upright `InputImage` through the
/// bitmap path; `orientation` applies as the EXIF value.
fn rgba_input<'local>(
    env: &mut Env<'local>,
    class: &Global<JClass<'static>>,
    rgba: &[u8],
    width: u32,
    height: u32,
    orientation: Orientation,
) -> Result<(JObject<'local>, i32), VisionError> {
    let data = env
        .byte_array_from_slice(rgba)
        .map_err(|error| VisionError::Platform(format!("encode texture pixels: {error}")))?;
    let bitmap = env
        .call_static_method(
            class,
            jni_str!("rgbaBitmap"),
            jni_sig!("([BIII)Landroid/graphics/Bitmap;"),
            &[
                JValue::Object(&data),
                JValue::Int(
                    i32::try_from(width).map_err(|error| {
                        VisionError::Platform(format!("texture width: {error}"))
                    })?,
                ),
                JValue::Int(
                    i32::try_from(height).map_err(|error| {
                        VisionError::Platform(format!("texture height: {error}"))
                    })?,
                ),
                JValue::Int(i32::from(orientation.exif())),
            ],
        )
        .and_then(JValueOwned::l)
        .map_err(|error| {
            VisionError::Platform(format!(
                "texture bitmap: {}",
                describe_jni_error(env, error)
            ))
        })?;
    bitmap_input(env, class, &bitmap)
}

/// Reads the `InputImage`'s detection-space dimensions and promotes it to a
/// global reference surviving the worker's frame.
fn finish_input(env: &mut Env<'_>, input: (JObject<'_>, i32)) -> Result<MlInput, VisionError> {
    let (input, rotation_degrees) = input;
    let width = env
        .call_method(&input, jni_str!("getWidth"), jni_sig!("()I"), &[])
        .and_then(JValueOwned::i)
        .map_err(|error| {
            VisionError::Platform(format!("input width: {}", describe_jni_error(env, error)))
        })?;
    let height = env
        .call_method(&input, jni_str!("getHeight"), jni_sig!("()I"), &[])
        .and_then(JValueOwned::i)
        .map_err(|error| {
            VisionError::Platform(format!("input height: {}", describe_jni_error(env, error)))
        })?;
    let input = env
        .new_global_ref(input)
        .map_err(|error| VisionError::Platform(format!("global ref for input: {error}")))?;
    Ok(MlInput {
        input,
        rotation_degrees,
        width: u32::try_from(width)
            .map_err(|error| VisionError::Platform(format!("input width: {error}")))?,
        height: u32::try_from(height)
            .map_err(|error| VisionError::Platform(format!("input height: {error}")))?,
    })
}

impl Preparation for SharedInput {
    #[cfg_attr(
        target_arch = "wasm32",
        expect(
            clippy::future_not_send,
            reason = "on wasm32 wgpu devices, queues and textures are not `Send`, so neither is a future holding them"
        )
    )]
    async fn prepare(context: Context<'_>, pixels: &Pixels) -> Result<Self, VisionError> {
        let pixels = pixels.clone();
        let device = Arc::clone(context.device());
        let queue = Arc::clone(context.queue());
        let input = on_vision_thread("waterkit-vision-input", move |env, context| {
            input_image(env, context, &pixels, &device, &queue)
        })
        .await?;
        Ok(Self(Arc::new(input)))
    }
}

/// Rotates a stored-space detection point into the upright image and
/// normalizes it.
// f64 intermediates keep the rotation exact; the normalized result wants the
// `Point`'s f32 fields, so the narrowing is intentional.
#[allow(clippy::cast_possible_truncation)]
fn normalize_point(x: i32, y: i32, rotation_degrees: i32, width: u32, height: u32) -> Point {
    let (x, y, width, height) = (
        f64::from(x),
        f64::from(y),
        f64::from(width),
        f64::from(height),
    );
    let (x, y) = match rotation_degrees {
        90 => (height - y, x),
        180 => (width - x, height - y),
        270 => (y, width - x),
        _ => (x, y),
    };
    let (width, height) = if rotation_degrees % 180 == 0 {
        (width, height)
    } else {
        (height, width)
    };
    Point {
        x: (x / width) as f32,
        y: (y / height) as f32,
    }
}

/// The quad of a detection's flat `x,y × 4` stored-space corner points.
pub fn quad(rotation_degrees: i32, width: u32, height: u32, points: &[i32]) -> Quad {
    let mut corners = [Point { x: 0.0, y: 0.0 }; 4];
    for (corner, pair) in corners.iter_mut().zip(points.as_chunks::<2>().0) {
        *corner = normalize_point(pair[0], pair[1], rotation_degrees, width, height);
    }
    Quad(corners)
}
