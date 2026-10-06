//! Android realization through Play services ML Kit (unbundled).
//!
//! The `play-services-mlkit-*` artifacts are thin clients: the detection
//! engines live in modules Play services delivers on demand, never in the
//! app. `VisionHelper.kt` wraps the client calls; `ModuleInstallClient`
//! reports which modules the device already has and fetches missing ones in
//! `prepare`.
//!
//! Image inputs reach ML Kit without a CPU copy whenever the platform allows:
//! a camera frame's `android.media.Image` (`YUV_420_888`, CPU-visible planes)
//! goes straight to `InputImage.fromMediaImage`; encoded bytes decode through
//! `BitmapFactory` with EXIF applied. Only a `wgpu` texture reads back to a
//! bitmap, as no public Android API accepts a GPU texture.

use std::sync::{Arc, OnceLock};

use futures::channel::oneshot;
use jni::objects::{Global, JIntArray, JObject, JValue, JValueOwned};
use jni::{Env, jni_sig, jni_str};
use waterkit_build::{
    AndroidError, DexHelper, describe_jni_error, dex_helper, with_android_context,
};

use crate::{
    Portable, RealizationSet, VisionCapabilities, VisionError,
    geometry::{Point, Quad},
    image::Pixels,
    sealed::{Context, Preparation},
};

#[cfg(feature = "camera")]
use crate::Orientation;
#[cfg(feature = "text")]
use crate::text::TextScript;

/// `waterkit.vision.VisionHelper`, compiled from this crate's
/// `kotlin-sources` and resolved through the application's class loader.
static HELPER: DexHelper = dex_helper!("waterkit.vision.VisionHelper");

impl From<AndroidError> for VisionError {
    fn from(error: AndroidError) -> Self {
        Self::Platform(error.to_string())
    }
}

// Module codes mirrored in `VisionHelper`: the index `moduleAvailability`
// reports and `prepareModule`/`recognizeText` take.
#[cfg(feature = "barcode")]
const MODULE_BARCODE: usize = 0;
#[cfg(feature = "text")]
const MODULE_SCRIPTS: [(usize, TextScript); 5] = [
    (1, TextScript::Latin),
    (2, TextScript::Chinese),
    (3, TextScript::Devanagari),
    (4, TextScript::Japanese),
    (5, TextScript::Korean),
];
#[cfg(any(feature = "barcode", feature = "text"))]
const MODULE_COUNT: usize = 6;

/// The `detect-barcodes` request name, for selection logging and errors.
#[cfg(feature = "barcode")]
const REQUEST_BARCODES: &str = "detect-barcodes";
/// The `recognize-text` request name.
#[cfg(feature = "text")]
const REQUEST_TEXT: &str = "recognize-text";

#[cfg(feature = "barcode")]
mod mlkit_format {
    use crate::Symbology;

    // `Barcode.FORMAT_*` constants, mirrored so the wire carries ML Kit
    // format ints rather than a private encoding.
    const FORMAT_CODE_128: i32 = 1;
    const FORMAT_CODE_39: i32 = 2;
    const FORMAT_CODE_93: i32 = 4;
    const FORMAT_CODABAR: i32 = 8;
    const FORMAT_DATA_MATRIX: i32 = 16;
    const FORMAT_EAN_13: i32 = 32;
    const FORMAT_EAN_8: i32 = 64;
    const FORMAT_ITF: i32 = 128;
    const FORMAT_QR_CODE: i32 = 256;
    const FORMAT_UPC_A: i32 = 512;
    const FORMAT_UPC_E: i32 = 1024;
    const FORMAT_PDF417: i32 = 2048;
    const FORMAT_AZTEC: i32 = 4096;

    /// Every symbology ML Kit's barcode engine can read, in its format order.
    pub const SERVED: &[(Symbology, i32)] = &[
        (Symbology::Aztec, FORMAT_AZTEC),
        (Symbology::Codabar, FORMAT_CODABAR),
        (Symbology::Code39, FORMAT_CODE_39),
        (Symbology::Code93, FORMAT_CODE_93),
        (Symbology::Code128, FORMAT_CODE_128),
        (Symbology::DataMatrix, FORMAT_DATA_MATRIX),
        (Symbology::Ean8, FORMAT_EAN_8),
        (Symbology::Ean13, FORMAT_EAN_13),
        (Symbology::Itf, FORMAT_ITF),
        (Symbology::Pdf417, FORMAT_PDF417),
        (Symbology::Qr, FORMAT_QR_CODE),
        (Symbology::UpcA, FORMAT_UPC_A),
        (Symbology::UpcE, FORMAT_UPC_E),
    ];

    /// The ML Kit format for `symbology`, when the engine serves it.
    pub fn of(symbology: Symbology) -> Option<i32> {
        SERVED
            .iter()
            .find_map(|(served, format)| (*served == symbology).then_some(*format))
    }

    /// The symbology a result's `format` names. ML Kit only reports formats
    /// it was asked for, so a miss means the engine or the wire disagrees.
    pub fn symbology(format: i32) -> Option<Symbology> {
        SERVED
            .iter()
            .find_map(|(served, served_format)| (*served_format == format).then_some(*served))
    }
}

#[cfg(feature = "barcode")]
use mlkit_format::{of as mlkit_format_of, symbology as symbology_of};

/// Whether Google Play services is usable on this device, probed once:
/// its presence never changes over the process' lifetime.
#[cfg(any(feature = "barcode", feature = "text"))]
fn play_services() -> Result<bool, VisionError> {
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
#[cfg(any(feature = "barcode", feature = "text"))]
async fn on_vision_thread<T, F>(label: &'static str, work: F) -> Result<T, VisionError>
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

/// Asks the helper which modules Play services currently has installed.
#[cfg(any(feature = "barcode", feature = "text"))]
async fn module_availability() -> Result<Vec<i32>, VisionError> {
    on_vision_thread("waterkit-vision-capabilities", |env, context| {
        let class = HELPER.class(env, context)?;
        let bits = env
            .call_static_method(
                class,
                jni_str!("moduleAvailability"),
                jni_sig!("(Landroid/content/Context;)[I"),
                &[JValue::Object(context)],
            )
            .and_then(JValueOwned::l)
            .map_err(|error| {
                VisionError::Platform(format!(
                    "module availability: {}",
                    describe_jni_error(env, error)
                ))
            })?;
        let bits = env.cast_local::<JIntArray>(bits).map_err(|error| {
            VisionError::Platform(format!("module availability array: {error}"))
        })?;
        let length = bits
            .len(env)
            .map_err(|error| VisionError::Platform(format!("module availability len: {error}")))?;
        let mut values = vec![0i32; length];
        bits.get_region(env, 0, &mut values)
            .map_err(|error| VisionError::Platform(format!("module availability read: {error}")))?;
        if values.len() != MODULE_COUNT {
            return Err(VisionError::Platform(format!(
                "module availability reported {} modules, expected {MODULE_COUNT}",
                values.len()
            )));
        }
        Ok(values)
    })
    .await
}

/// Fetches the module serving `module`, when Play services does not already
/// have it.
#[cfg(any(feature = "barcode", feature = "text"))]
async fn prepare_module(module: usize) -> Result<(), VisionError> {
    let code = i32::try_from(module)
        .map_err(|error| VisionError::Platform(format!("module code: {error}")))?;
    on_vision_thread("waterkit-vision-prepare", move |env, context| {
        let class = HELPER.class(env, context)?;
        env.call_static_method(
            class,
            jni_str!("prepareModule"),
            jni_sig!("(Landroid/content/Context;I)V"),
            &[JValue::Object(context), JValue::Int(code)],
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
#[cfg(any(feature = "barcode", feature = "text"))]
#[derive(Debug)]
struct MlInput {
    /// The `InputImage` this pass' requests all read.
    input: Global<JObject<'static>>,
    /// The clockwise degrees detections are reported against; the points
    /// arrive in the stored orientation, and this rotation maps them to
    /// upright space.
    rotation_degrees: i32,
    /// Detection-space dimensions, matching `rotation_degrees`.
    width: u32,
    height: u32,
}

/// `Pass`-shared [`MlInput`], Arc'd so each request's worker thread takes
/// its own handle.
#[cfg(any(feature = "barcode", feature = "text"))]
#[derive(Debug)]
struct SharedInput(Arc<MlInput>);

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
/// `Pixels::Texture` is the only CPU-copied input path: ML Kit accepts
/// `android.media.Image`, `Bitmap`, bytes or a file, and a wgpu texture is
/// none of those.
#[cfg(any(feature = "barcode", feature = "text"))]
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
    if !texture.usage().contains(wgpu::TextureUsages::COPY_SRC) {
        return Err(VisionError::Unsupported(String::from(
            "the image texture lacks COPY_SRC, so its pixels cannot reach ML Kit",
        )));
    }
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
#[cfg(any(feature = "barcode", feature = "text"))]
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
            let data = env.byte_array_from_slice(&rgba).map_err(|error| {
                VisionError::Platform(format!("encode texture pixels: {error}"))
            })?;
            let bitmap = env
                .call_static_method(
                    class,
                    jni_str!("rgbaBitmap"),
                    jni_sig!("([BIII)Landroid/graphics/Bitmap;"),
                    &[
                        JValue::Object(&data),
                        JValue::Int(i32::try_from(width).map_err(|error| {
                            VisionError::Platform(format!("texture width: {error}"))
                        })?),
                        JValue::Int(i32::try_from(height).map_err(|error| {
                            VisionError::Platform(format!("texture height: {error}"))
                        })?),
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
            bitmap_input(env, class, &bitmap)?
        }
        #[cfg(feature = "camera")]
        Pixels::Frame {
            orientation, media, ..
        } => {
            let rotation = rotation_degrees(*orientation)?;
            let image = env
                .call_static_method(
                    class,
                    jni_str!("mediaInput"),
                    jni_sig!("(Landroid/media/Image;I)Lcom/google/mlkit/vision/common/InputImage;"),
                    &[
                        JValue::Object(media.image().as_obj()),
                        JValue::Int(rotation),
                    ],
                )
                .and_then(JValueOwned::l)
                .map_err(|error| {
                    VisionError::Platform(format!(
                        "frame input: {}",
                        describe_jni_error(env, error)
                    ))
                })?;
            (image, rotation)
        }
    };
    finish_input(env, input)
}

/// Wraps `bitmap` — already upright — as an `InputImage`.
#[cfg(any(feature = "barcode", feature = "text"))]
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

/// Reads the `InputImage`'s detection-space dimensions and promotes it to a
/// global reference surviving the worker's frame.
#[cfg(any(feature = "barcode", feature = "text"))]
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

#[cfg(any(feature = "barcode", feature = "text"))]
impl Preparation for SharedInput {
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
#[cfg(any(feature = "barcode", feature = "text"))]
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
#[cfg(any(feature = "barcode", feature = "text"))]
fn quad(rotation_degrees: i32, width: u32, height: u32, points: &[i32]) -> Quad {
    let mut corners = [Point { x: 0.0, y: 0.0 }; 4];
    for (corner, pair) in corners.iter_mut().zip(points.as_chunks::<2>().0) {
        *corner = normalize_point(pair[0], pair[1], rotation_degrees, width, height);
    }
    Quad(corners)
}

/// The native capabilities this device reports. A probe failure reports the
/// same empty sets a device without Play services gives; `prepare` and
/// `perform` surface the real error.
#[cfg(any(feature = "barcode", feature = "text"))]
pub async fn capabilities() -> VisionCapabilities {
    let bits = match module_availability().await {
        Ok(bits) => bits,
        Err(error) => {
            tracing::warn!(
                %error,
                "waterkit-vision: module availability probe failed; reporting no native realization"
            );
            vec![0; MODULE_COUNT]
        }
    };
    VisionCapabilities {
        #[cfg(feature = "barcode")]
        barcodes: RealizationSet {
            native: if bits[MODULE_BARCODE] != 0 {
                mlkit_format::SERVED.iter().map(|(s, _)| *s).collect()
            } else {
                enumset::EnumSet::empty()
            },
            portable: Portable::Absent,
        },
        #[cfg(feature = "text")]
        text: RealizationSet {
            native: MODULE_SCRIPTS
                .iter()
                .filter(|(module, _)| bits[*module] != 0)
                .map(|(_, script)| script.identifier())
                .collect(),
            portable: Portable::Absent,
        },
    }
}

/// The native capabilities this device reports when no request feature is
/// enabled: none are compiled.
#[cfg(not(any(feature = "barcode", feature = "text")))]
pub async fn capabilities() -> VisionCapabilities {
    VisionCapabilities {}
}

#[cfg(feature = "barcode")]
mod barcodes {
    use std::sync::Arc;

    use bytes::Bytes;
    use jni::objects::{JByteArray, JIntArray, JObject, JObjectArray, JValue, JValueOwned};
    use jni::{Env, jni_sig, jni_str};
    use waterkit_build::describe_jni_error;

    use super::{
        HELPER, MODULE_BARCODE, MlInput, REQUEST_BARCODES, SharedInput, mlkit_format,
        mlkit_format_of, on_vision_thread, play_services, prepare_module, quad, symbology_of,
    };
    use crate::{
        Barcode, DetectBarcodes, Payload, Symbology, VisionError,
        sealed::{Context, Offer, Pass, Plan, Realization},
    };

    /// The symbologies this realization serves, minus the request's: an
    /// empty difference means the engine reads everything asked.
    fn unserved(symbologies: enumset::EnumSet<Symbology>) -> Vec<Symbology> {
        let served: enumset::EnumSet<Symbology> =
            mlkit_format::SERVED.iter().map(|(s, _)| *s).collect();
        (symbologies - served).iter().collect()
    }

    /// ML Kit serves the request's symbologies when they all map to its
    /// formats and Play services is usable; the barcode module itself is
    /// delivered to Play services on `prepare`.
    fn native_offer(symbologies: enumset::EnumSet<Symbology>) -> Result<Offer, VisionError> {
        let missing = unserved(symbologies);
        if !missing.is_empty() {
            return Ok(Offer::Lacks(format!(
                "symbologies {}",
                missing
                    .iter()
                    .map(|s| format!("{s:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        Ok(if play_services()? {
            Offer::Serves
        } else {
            Offer::Absent
        })
    }

    /// Barcode detection over the shared [`MlInput`].
    #[derive(Debug)]
    pub struct BarcodePlan {
        realization: Realization,
        formats: Vec<i32>,
    }

    pub fn plan_barcodes(
        context: Context<'_>,
        symbologies: enumset::EnumSet<Symbology>,
    ) -> Result<BarcodePlan, VisionError> {
        let native = native_offer(symbologies)?;
        let realization = context.select(REQUEST_BARCODES, &native, &Offer::Absent)?;
        debug_assert_eq!(realization, Realization::Native);
        let formats = symbologies
            .iter()
            .map(|symbology| {
                mlkit_format_of(symbology)
                    .expect("the offer rejected symbologies ML Kit cannot serve")
            })
            .collect();
        Ok(BarcodePlan {
            realization,
            formats,
        })
    }

    /// Reads one `BarcodeRow` into a [`Barcode`].
    fn barcode_row(
        env: &mut Env<'_>,
        row: &JObject<'_>,
        input: &MlInput,
    ) -> Result<Barcode, VisionError> {
        let format = env
            .get_field(row, jni_str!("format"), jni_sig!("I"))
            .and_then(JValueOwned::i)
            .map_err(|error| VisionError::Platform(format!("barcode format: {error}")))?;
        let symbology = symbology_of(format).ok_or_else(|| {
            VisionError::Platform(format!("barcode reported unknown format {format}"))
        })?;
        let bytes = env
            .get_field(row, jni_str!("bytes"), jni_sig!("[B"))
            .and_then(JValueOwned::l)
            .map_err(|error| VisionError::Platform(format!("barcode bytes: {error}")))?;
        let bytes = env
            .cast_local::<JByteArray>(bytes)
            .map_err(|error| VisionError::Platform(format!("barcode bytes cast: {error}")))?;
        let bytes = env
            .convert_byte_array(&bytes)
            .map_err(|error| VisionError::Platform(format!("barcode bytes read: {error}")))?;
        let points = env
            .get_field(row, jni_str!("points"), jni_sig!("[I"))
            .and_then(JValueOwned::l)
            .map_err(|error| VisionError::Platform(format!("barcode points: {error}")))?;
        let points = env
            .cast_local::<JIntArray>(points)
            .map_err(|error| VisionError::Platform(format!("barcode points cast: {error}")))?;
        let length = points
            .len(env)
            .map_err(|error| VisionError::Platform(format!("barcode points len: {error}")))?;
        let mut flat = vec![0i32; length];
        points
            .get_region(env, 0, &mut flat)
            .map_err(|error| VisionError::Platform(format!("barcode points read: {error}")))?;
        Ok(Barcode {
            symbology,
            payload: Payload {
                bytes: Bytes::from(bytes),
            },
            bounds: quad(input.rotation_degrees, input.width, input.height, &flat),
        })
    }

    impl Plan<DetectBarcodes> for BarcodePlan {
        async fn prepare(&self, _context: Context<'_>) -> Result<(), VisionError> {
            debug_assert_eq!(self.realization, Realization::Native);
            prepare_module(MODULE_BARCODE).await
        }

        async fn run(self, pass: &mut Pass<'_>) -> Result<Vec<Barcode>, VisionError> {
            debug_assert_eq!(self.realization, Realization::Native);
            let input = Arc::clone(&pass.prepared::<SharedInput>().await?.0);
            let formats = self.formats;
            on_vision_thread("waterkit-vision-barcode", move |env, context| {
                let class = HELPER.class(env, context)?;
                let formats_array = JIntArray::new(env, formats.len()).map_err(|error| {
                    VisionError::Platform(format!("formats array: {error}"))
                })?;
                formats_array
                    .set_region(env, 0, &formats)
                    .map_err(|error| {
                        VisionError::Platform(format!("formats array: {error}"))
                    })?;
                let rows = env
                    .call_static_method(
                        class,
                        jni_str!("detectBarcodes"),
                        jni_sig!(
                            "(Lcom/google/mlkit/vision/common/InputImage;[I)[Lwaterkit/vision/VisionHelper$BarcodeRow;"
                        ),
                        &[
                            JValue::Object(input.input.as_obj()),
                            JValue::Object(&formats_array),
                        ],
                    )
                    .and_then(JValueOwned::l)
                    .map_err(|error| {
                        VisionError::Platform(format!(
                            "detect barcodes: {}",
                            describe_jni_error(env, error)
                        ))
                    })?;
                let rows = env
                    .cast_local::<JObjectArray<JObject>>(rows)
                    .map_err(|error| {
                        VisionError::Platform(format!("barcode rows cast: {error}"))
                    })?;
                let count = rows
                    .len(env)
                    .map_err(|error| VisionError::Platform(format!("barcode rows len: {error}")))?;
                let mut barcodes = Vec::with_capacity(count);
                for index in 0..count {
                    let row = rows.get_element(env, index).map_err(|error| {
                        VisionError::Platform(format!("barcode row {index}: {error}"))
                    })?;
                    barcodes.push(barcode_row(env, &row, &input)?);
                }
                Ok(barcodes)
            })
            .await
        }
    }
}

#[cfg(feature = "barcode")]
pub use barcodes::*;

#[cfg(feature = "text")]
mod texts {
    use std::sync::Arc;

    use jni::objects::{JIntArray, JObject, JObjectArray, JValue, JValueOwned};
    use jni::{Env, jni_sig, jni_str};
    use waterkit_build::{decode_string, describe_jni_error};

    use super::{
        HELPER, MlInput, REQUEST_TEXT, SharedInput, on_vision_thread, play_services,
        prepare_module, quad,
    };
    use crate::{
        TextLine, VisionError,
        sealed::{Context, Offer, Pass, Plan, Realization},
        text::{RecognizeText, TextScript},
    };

    impl TextScript {
        /// The `VisionHelper` module code for the script.
        const fn module(self) -> usize {
            match self {
                Self::Latin => 1,
                Self::Chinese => 2,
                Self::Devanagari => 3,
                Self::Japanese => 4,
                Self::Korean => 5,
            }
        }
    }

    /// Text recognition over the shared [`MlInput`].
    #[derive(Debug)]
    pub struct TextPlan {
        realization: Realization,
        script: TextScript,
    }

    pub fn plan_text(
        context: Context<'_>,
        request: &RecognizeText,
    ) -> Result<TextPlan, VisionError> {
        let script = request.script().map_err(|failed| {
            VisionError::Unsupported(format!(
                "recognize-text: languages [{}] resolve to no single served script",
                failed.join(", ")
            ))
        })?;
        let native = if play_services()? {
            Offer::Serves
        } else {
            Offer::Absent
        };
        let realization = context.select(REQUEST_TEXT, &native, &Offer::Absent)?;
        debug_assert_eq!(realization, Realization::Native);
        Ok(TextPlan {
            realization,
            script,
        })
    }

    /// Reads one `TextRow` into a [`TextLine`].
    fn text_row(
        env: &mut Env<'_>,
        row: &JObject<'_>,
        input: &MlInput,
    ) -> Result<TextLine, VisionError> {
        let text = env
            .get_field(row, jni_str!("text"), jni_sig!("Ljava/lang/String;"))
            .and_then(JValueOwned::l)
            .map_err(|error| VisionError::Platform(format!("line text: {error}")))?;
        let text = decode_string(env, &text)
            .map_err(|error| VisionError::Platform(format!("line text: {error}")))?;
        let confidence = env
            .get_field(row, jni_str!("confidence"), jni_sig!("F"))
            .and_then(JValueOwned::f)
            .map_err(|error| VisionError::Platform(format!("line confidence: {error}")))?;
        let points = env
            .get_field(row, jni_str!("points"), jni_sig!("[I"))
            .and_then(JValueOwned::l)
            .map_err(|error| VisionError::Platform(format!("line points: {error}")))?;
        let points = env
            .cast_local::<JIntArray>(points)
            .map_err(|error| VisionError::Platform(format!("line points cast: {error}")))?;
        let length = points
            .len(env)
            .map_err(|error| VisionError::Platform(format!("line points len: {error}")))?;
        let mut flat = vec![0i32; length];
        points
            .get_region(env, 0, &mut flat)
            .map_err(|error| VisionError::Platform(format!("line points read: {error}")))?;
        Ok(TextLine {
            text,
            confidence,
            bounds: quad(input.rotation_degrees, input.width, input.height, &flat),
        })
    }

    impl Plan<RecognizeText> for TextPlan {
        async fn prepare(&self, _context: Context<'_>) -> Result<(), VisionError> {
            debug_assert_eq!(self.realization, Realization::Native);
            prepare_module(self.script.module()).await
        }

        async fn run(self, pass: &mut Pass<'_>) -> Result<Vec<TextLine>, VisionError> {
            debug_assert_eq!(self.realization, Realization::Native);
            let input = Arc::clone(&pass.prepared::<SharedInput>().await?.0);
            let script = i32::try_from(self.script.module())
                .map_err(|error| VisionError::Platform(format!("script module code: {error}")))?;
            on_vision_thread("waterkit-vision-text", move |env, context| {
                let class = HELPER.class(env, context)?;
                let rows = env
                    .call_static_method(
                        class,
                        jni_str!("recognizeText"),
                        jni_sig!(
                            "(Lcom/google/mlkit/vision/common/InputImage;I)[Lwaterkit/vision/VisionHelper$TextRow;"
                        ),
                        &[JValue::Object(input.input.as_obj()), JValue::Int(script)],
                    )
                    .and_then(JValueOwned::l)
                    .map_err(|error| {
                        VisionError::Platform(format!(
                            "recognize text: {}",
                            describe_jni_error(env, error)
                        ))
                    })?;
                let rows = env
                    .cast_local::<JObjectArray<JObject>>(rows)
                    .map_err(|error| {
                        VisionError::Platform(format!("text rows cast: {error}"))
                    })?;
                let count = rows
                    .len(env)
                    .map_err(|error| VisionError::Platform(format!("text rows len: {error}")))?;
                let mut lines = Vec::with_capacity(count);
                for index in 0..count {
                    let row = rows.get_element(env, index).map_err(|error| {
                        VisionError::Platform(format!("text row {index}: {error}"))
                    })?;
                    lines.push(text_row(env, &row, &input)?);
                }
                Ok(lines)
            })
            .await
        }
    }
}

#[cfg(feature = "text")]
pub use texts::*;
