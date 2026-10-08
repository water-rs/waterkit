//! Shared plumbing for the `barcode` and `text` requests' native
//! realization: Play services ML Kit, unbundled.
//!
//! The `play-services-mlkit-*` artifacts are thin clients: the detection
//! engines live in modules Play services delivers on demand, never in the
//! app. `VisionHelper.kt` wraps the client calls; `ModuleInstallClient`
//! installs a missing module in `prepare`.
//!
//! Image inputs reach ML Kit without a CPU copy whenever the platform
//! allows: encoded bytes decode through `BitmapFactory` with EXIF applied,
//! and a camera frame's luma plane reads back into an `NV21` byte array —
//! ML Kit works on luminance, so chroma is never touched. Only a `wgpu`
//! texture reads back RGBA into a bitmap, as no public Android API accepts
//! a GPU texture.

use std::sync::{Arc, OnceLock};

use futures::channel::oneshot;
use jni::objects::{Global, JObject, JValue, JValueOwned};
use jni::{Env, jni_sig, jni_str};
use waterkit_build::{DexHelper, describe_jni_error, dex_helper, with_android_context};

use crate::{
    Orientation, VisionError,
    geometry::{Point, Quad},
    image::Pixels,
    sealed::{Context, Preparation},
};

/// `waterkit.vision.VisionHelper`, compiled from this crate's
/// `kotlin-sources` and resolved through the application's class loader.
pub static HELPER: DexHelper = dex_helper!("waterkit.vision.VisionHelper");

// Module codes mirrored in `VisionHelper`: what `prepareModule` and
// `recognizeText` take.
#[cfg(feature = "barcode")]
pub const MODULE_BARCODE: i32 = 0;
#[cfg(feature = "text")]
pub const MODULE_LATIN: i32 = 1;
#[cfg(feature = "text")]
pub const MODULE_CHINESE: i32 = 2;
#[cfg(feature = "text")]
pub const MODULE_DEVANAGARI: i32 = 3;
#[cfg(feature = "text")]
pub const MODULE_JAPANESE: i32 = 4;
#[cfg(feature = "text")]
pub const MODULE_KOREAN: i32 = 5;

/// Whether Google Play services is usable on this device, probed once: its
/// presence never changes over the process' lifetime.
pub fn play_services() -> Result<bool, VisionError> {
    static PLAY_SERVICES: OnceLock<bool> = OnceLock::new();
    if let Some(available) = PLAY_SERVICES.get() {
        return Ok(*available);
    }
    let available = with_android_context(|env, context| {
        let class = HELPER.class(env, context)?;
        env.call_static_method(
            class,
            jni_str!("hasGooglePlayServices"),
            jni_sig!("(Landroid/content/Context;)Z"),
            &[JValue::Object(context)],
        )
        .and_then(JValueOwned::z)
        .map_err(|error| {
            VisionError::Platform(format!(
                "probe Play services: {}",
                describe_jni_error(env, error)
            ))
        })
    })?;
    tracing::debug!(available, "waterkit-vision: probed Play services");
    Ok(*PLAY_SERVICES.get_or_init(|| available))
}

/// Runs `work` with the Android context on a dedicated thread, so the JNI
/// calls and the helper's `Tasks.await` never block the awaiting task.
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

/// Fetches the module serving `module`, when Play services does not already
/// have it.
pub async fn prepare_module(module: i32) -> Result<(), VisionError> {
    on_vision_thread("waterkit-vision-prepare", move |env, context| {
        let class = HELPER.class(env, context)?;
        env.call_static_method(
            class,
            jni_str!("prepareModule"),
            jni_sig!("(Landroid/content/Context;I)V"),
            &[JValue::Object(context), JValue::Int(module)],
        )
        .map_err(|error| {
            VisionError::ModelUnavailable(format!(
                "module {module} install: {}",
                describe_jni_error(env, error)
            ))
        })?;
        Ok(())
    })
    .await
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

/// Reads `texture`'s `aspect` plane back into tightly packed bytes of
/// `bytes_per_pixel`.
fn read_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    bytes_per_pixel: u32,
    aspect: wgpu::TextureAspect,
) -> Result<(Vec<u8>, u32, u32), VisionError> {
    if !texture.usage().contains(wgpu::TextureUsages::COPY_SRC) {
        return Err(VisionError::Unsupported(String::from(
            "the image texture lacks COPY_SRC, so its pixels cannot reach ML Kit",
        )));
    }
    let size = texture.size();
    let row_bytes = size.width * bytes_per_pixel;
    let padded_row = row_bytes.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("waterkit-vision texture readback"),
        size: u64::from(padded_row * size.height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect,
        },
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
    let bytes: Vec<u8> = mapped
        .chunks(padded_row as usize)
        .flat_map(|row| &row[..row_bytes as usize])
        .copied()
        .collect();
    drop(mapped);
    buffer.unmap();
    Ok((bytes, size.width, size.height))
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
    let swizzle_bgra = match texture.format() {
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => false,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => true,
        format => {
            return Err(VisionError::Unsupported(format!(
                "ML Kit inputs need an 8-bit RGBA texture, not {format:?}"
            )));
        }
    };
    let (mut rgba, width, height) =
        read_texture(device, queue, texture, 4, wgpu::TextureAspect::All)?;
    if swizzle_bgra {
        rgba.as_chunks_mut::<4>().0.iter_mut().for_each(|pixel| {
            pixel.swap(0, 2);
        });
    }
    Ok((rgba, width, height))
}

/// One luma byte per pixel: `R8` planes copy as-is, the `luma` view of an
/// `NV12` texture that aliases the camera's buffer reads its first plane,
/// `R16` planes (P010) keep each code's most significant byte, and
/// `YCbCr422`'s packed YUYV texture keeps the first and third byte of each
/// four-byte texel (Y'0, Y'1).
///
/// The `NV21` chroma plane is neutral — ML Kit's detectors read luminance,
/// so chroma is never touched.
#[cfg(feature = "camera")]
fn luma_nv21(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    view: &wgpu::TextureView,
) -> Result<(Vec<u8>, u32, u32), VisionError> {
    let texture = view.texture();
    let (bytes, width, height) = match texture.format() {
        wgpu::TextureFormat::R8Unorm => {
            read_texture(device, queue, texture, 1, wgpu::TextureAspect::All)?
        }
        wgpu::TextureFormat::NV12 => {
            read_texture(device, queue, texture, 1, wgpu::TextureAspect::Plane0)?
        }
        wgpu::TextureFormat::R16Unorm => {
            let (bytes, width, height) =
                read_texture(device, queue, texture, 2, wgpu::TextureAspect::All)?;
            (
                bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|code| code[1])
                    .collect(),
                width,
                height,
            )
        }
        wgpu::TextureFormat::Rgba8Unorm => {
            let (bytes, texture_width, height) =
                read_texture(device, queue, texture, 4, wgpu::TextureAspect::All)?;
            (
                bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|texel| [texel[0], texel[2]])
                    .collect(),
                // A YUYV texel packs two horizontally adjacent pixels.
                texture_width * 2,
                height,
            )
        }
        format => {
            return Err(VisionError::Unsupported(format!(
                "a camera plane formatted {format:?} cannot feed ML Kit"
            )));
        }
    };
    let chroma_len = (width.div_ceil(2) * height.div_ceil(2) * 2) as usize;
    let mut nv21 = bytes;
    nv21.resize(nv21.len() + chroma_len, 0x80);
    Ok((nv21, width, height))
}

/// Builds an `InputImage` from an `NV21` byte array and its stored-space
/// `rotation`.
#[cfg(feature = "camera")]
fn nv21_input<'local>(
    env: &mut Env<'local>,
    class: &Global<jni::objects::JClass<'static>>,
    nv21: &[u8],
    width: u32,
    height: u32,
    rotation: i32,
) -> Result<(JObject<'local>, i32), VisionError> {
    let data = env
        .byte_array_from_slice(nv21)
        .map_err(|error| VisionError::Platform(format!("encode frame luma: {error}")))?;
    let image = env
        .call_static_method(
            class,
            jni_str!("nv21Input"),
            jni_sig!("([BIII)Lcom/google/mlkit/vision/common/InputImage;"),
            &[
                JValue::Object(&data),
                JValue::Int(
                    i32::try_from(width)
                        .map_err(|error| VisionError::Platform(format!("frame width: {error}")))?,
                ),
                JValue::Int(
                    i32::try_from(height)
                        .map_err(|error| VisionError::Platform(format!("frame height: {error}")))?,
                ),
                JValue::Int(rotation),
            ],
        )
        .and_then(JValueOwned::l)
        .map_err(|error| {
            VisionError::Platform(format!("frame input: {}", describe_jni_error(env, error)))
        })?;
    Ok((image, rotation))
}

/// Builds the pass' `InputImage` from `pixels` on the worker thread.
fn input_image(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    pixels: &Pixels,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> Result<MlInput, VisionError> {
    let class = HELPER.class(env, context)?;
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
        Pixels::Frame {
            planes,
            orientation,
            ..
        } => match planes {
            waterkit_camera::FramePlanes::Rgb(view) => {
                let (rgba, width, height) = readback_rgba(device, queue, view.texture())?;
                rgba_input(env, class, &rgba, width, height, *orientation)?
            }
            waterkit_camera::FramePlanes::YCbCr420 { luma, .. } => {
                let rotation = rotation_degrees(*orientation)?;
                let (nv21, width, height) = luma_nv21(device, queue, luma)?;
                nv21_input(env, class, &nv21, width, height, rotation)?
            }
            waterkit_camera::FramePlanes::YCbCr422 { yuyv } => {
                let rotation = rotation_degrees(*orientation)?;
                let (nv21, width, height) = luma_nv21(device, queue, yuyv)?;
                nv21_input(env, class, &nv21, width, height, rotation)?
            }
        },
    };
    finish_input(env, input)
}

/// Wraps `bitmap` — already upright — as an `InputImage`.
fn bitmap_input<'local>(
    env: &mut Env<'local>,
    class: &Global<jni::objects::JClass<'static>>,
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
    class: &Global<jni::objects::JClass<'static>>,
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
    let (w, h) = (f64::from(width), f64::from(height));
    let (upright_x, upright_y) = match rotation_degrees {
        90 => (h - f64::from(y), f64::from(x)),
        180 => (w - f64::from(x), h - f64::from(y)),
        270 => (f64::from(y), w - f64::from(x)),
        _ => (f64::from(x), f64::from(y)),
    };
    let (upright_w, upright_h) = if rotation_degrees % 180 == 90 {
        (h, w)
    } else {
        (w, h)
    };
    Point {
        x: (upright_x / upright_w) as f32,
        y: (upright_y / upright_h) as f32,
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
