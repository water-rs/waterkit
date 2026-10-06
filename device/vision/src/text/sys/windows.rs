//! The native text realization: `Windows.Media.Ocr` bridged through
//! `windows-rs`.
//!
//! `OcrEngine` reads [`SoftwareBitmap`]s. Frame pixels are read back through
//! wgpu once — `SoftwareBitmap::CreateCopyFromSurfaceAsync` consumes a D3D11
//! `IDirect3DSurface`, and reaching it from a wgpu D3D12 texture requires
//! `ID3D11On12` plumbing this crate's `forbid(unsafe_code)` excludes. The
//! readback's cost is measured and logged at debug level.

use std::{future::poll_fn, task::Poll, time::Instant};

use bytes::Bytes;
use icu_locale_core::LanguageIdentifier;
use windows::{
    Globalization::Language,
    Graphics::Imaging::{
        BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat, BitmapTransform, ColorManagementMode,
        ExifOrientationMode, SoftwareBitmap,
    },
    Media::Ocr::{OcrEngine, OcrLine, OcrResult},
    Storage::Streams::{DataWriter, InMemoryRandomAccessStream},
    System::UserProfile::GlobalizationPreferences,
    core::HSTRING,
};

use crate::{
    Orientation, Point, Quad, TextLine, TextWord, VisionError,
    image::Pixels,
    sealed::{Context, Offer, Pass, Preparation},
    text::{RecognizeText, TextPlan},
};

fn platform(context: &'static str) -> impl Fn(windows::core::Error) -> VisionError {
    move |error| VisionError::Platform(format!("{context}: {error}"))
}

/// `OcrEngine::AvailableRecognizerLanguages`, listed exactly.
///
/// An enumeration failure or a tag ICU cannot parse is logged and skipped —
/// the caller still sees every language the OS did return.
pub fn recognizer_languages() -> Vec<LanguageIdentifier> {
    match OcrEngine::AvailableRecognizerLanguages() {
        Ok(languages) => languages
            .into_iter()
            .filter_map(|language| match language.LanguageTag() {
                Ok(tag) => match LanguageIdentifier::try_from_str(&tag.to_string()) {
                    Ok(identifier) => Some(identifier),
                    Err(error) => {
                        tracing::warn!(
                            tag = %tag,
                            %error,
                            "Windows.Media.Ocr returned an unparsable language tag"
                        );
                        None
                    }
                },
                Err(error) => {
                    tracing::warn!(%error, "Windows.Media.Ocr language has no tag");
                    None
                }
            })
            .collect(),
        Err(error) => {
            tracing::warn!(%error, "Windows.Media.Ocr language enumeration failed");
            Vec::new()
        }
    }
}

fn language_supported(identifier: &LanguageIdentifier) -> Result<bool, VisionError> {
    let tag = identifier.to_string();
    let language = Language::CreateLanguage(&HSTRING::from(&*tag))
        .map_err(platform("Globalization::Language::CreateLanguage"))?;
    OcrEngine::IsLanguageSupported(&language).map_err(platform("OcrEngine::IsLanguageSupported"))
}

/// Whether a recognizer supports any of the user's profile languages.
fn profile_supported() -> Result<bool, VisionError> {
    let languages = GlobalizationPreferences::Languages()
        .map_err(platform("GlobalizationPreferences::Languages"))?;
    for tag in &languages {
        let language = Language::CreateLanguage(&tag)
            .map_err(platform("Globalization::Language::CreateLanguage"))?;
        if OcrEngine::IsLanguageSupported(&language)
            .map_err(platform("OcrEngine::IsLanguageSupported"))?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// What the native realization offers for `request`.
///
/// One `OcrEngine` recognizes one language per image, so a request naming
/// several languages can never be served natively.
pub fn offer(request: &RecognizeText) -> Offer {
    match request.languages.as_slice() {
        [] => match profile_supported() {
            Ok(true) => Offer::Serves,
            Ok(false) => Offer::Lacks("a recognizer language matching the user profile".to_owned()),
            Err(error) => Offer::Lacks(error.to_string()),
        },
        [language] => match language_supported(language) {
            Ok(true) => Offer::Serves,
            Ok(false) => Offer::Lacks(format!("recognizer language {language}")),
            Err(error) => Offer::Lacks(error.to_string()),
        },
        many => Offer::Lacks(format!(
            "one recognizer language at a time ({} requested)",
            many.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Verifies that an engine exists for the selected languages.
pub fn prepare(plan: &TextPlan) -> Result<(), VisionError> {
    engine(&plan.languages).map(|_| ())
}

fn engine(languages: &[LanguageIdentifier]) -> Result<OcrEngine, VisionError> {
    match languages {
        [] => OcrEngine::TryCreateFromUserProfileLanguages(),
        [language] => OcrEngine::TryCreateFromLanguage(
            &Language::CreateLanguage(&HSTRING::from(language.to_string()))
                .map_err(platform("Globalization::Language::CreateLanguage"))?,
        ),
        many => unreachable!(
            "multi-language requests ({}) are declined at selection",
            many.len()
        ),
    }
    .map_err(platform("OcrEngine::TryCreateFrom*"))
}

/// Runs the engine over the pass's shared bitmap and maps the result.
pub async fn recognize(pass: &mut Pass<'_>, plan: &TextPlan) -> Result<Vec<TextLine>, VisionError> {
    tracing::debug!(
        level = ?plan.level,
        "recognizing text with Windows.Media.Ocr"
    );
    let engine = engine(&plan.languages)?;
    let prepared = pass.prepared::<PreparedBitmap>().await?;
    let result = engine
        .RecognizeAsync(&prepared.bitmap)
        .map_err(platform("OcrEngine::RecognizeAsync"))?
        .await
        .map_err(platform("OcrEngine::RecognizeAsync"))?;
    lines(&result, prepared.width, prepared.height)
}

fn lines(result: &OcrResult, width: u32, height: u32) -> Result<Vec<TextLine>, VisionError> {
    result
        .Lines()
        .map_err(platform("OcrResult::Lines"))?
        .into_iter()
        .map(|line| text_line(&line, width, height))
        .collect()
}

fn text_line(line: &OcrLine, width: u32, height: u32) -> Result<TextLine, VisionError> {
    let text = line.Text().map_err(platform("OcrLine::Text"))?.to_string();
    let mut words = Vec::new();
    let mut x0 = f32::INFINITY;
    let mut y0 = f32::INFINITY;
    let mut x1 = f32::NEG_INFINITY;
    let mut y1 = f32::NEG_INFINITY;
    for word in &line.Words().map_err(platform("OcrLine::Words"))? {
        let bounds = quad(
            &word
                .BoundingRect()
                .map_err(platform("OcrWord::BoundingRect"))?,
            width,
            height,
        );
        for point in bounds.0 {
            x0 = x0.min(point.x);
            y0 = y0.min(point.y);
            x1 = x1.max(point.x);
            y1 = y1.max(point.y);
        }
        words.push(TextWord {
            text: word.Text().map_err(platform("OcrWord::Text"))?.to_string(),
            confidence: None,
            bounds,
        });
    }
    let bounds = if words.is_empty() {
        Quad([Point { x: 0.0, y: 0.0 }; 4])
    } else {
        Quad([
            Point { x: x0, y: y0 },
            Point { x: x1, y: y0 },
            Point { x: x1, y: y1 },
            Point { x: x0, y: y1 },
        ])
    };
    Ok(TextLine {
        text,
        confidence: None,
        bounds,
        words,
    })
}

/// A word's `Rect` normalized into the upright image's `Quad`.
///
/// `Rect` values are single-precision and image dimensions fit in an
/// `f32`'s mantissa for any real bitmap, so division stays in `f32`.
#[expect(
    clippy::cast_precision_loss,
    reason = "bitmap dimensions are small enough to be exact in f32"
)]
fn quad(rect: &windows::Foundation::Rect, width: u32, height: u32) -> Quad {
    let x0 = rect.X / width as f32;
    let y0 = rect.Y / height as f32;
    let x1 = x0 + rect.Width / width as f32;
    let y1 = y0 + rect.Height / height as f32;
    Quad([
        Point { x: x0, y: y0 },
        Point { x: x1, y: y0 },
        Point { x: x1, y: y1 },
        Point { x: x0, y: y1 },
    ])
}

/// The pass-shared [`SoftwareBitmap`], upright and within the engine's size
/// limit.
#[derive(Debug)]
struct PreparedBitmap {
    bitmap: SoftwareBitmap,
    width: u32,
    height: u32,
}

impl Preparation for PreparedBitmap {
    #[cfg_attr(
        target_arch = "wasm32",
        expect(
            clippy::future_not_send,
            reason = "on wasm32 wgpu devices, queues and textures are not `Send`, so neither is a future holding them"
        )
    )]
    async fn prepare(context: Context<'_>, pixels: &Pixels) -> Result<Self, VisionError> {
        let bitmap = match pixels {
            Pixels::Texture {
                texture,
                orientation,
            } => {
                let raster = read_raster(
                    texture,
                    Texel::Direct,
                    *orientation,
                    context.device(),
                    context.queue(),
                )
                .await?;
                bitmap_from_raster(&raster)?
            }
            #[cfg(feature = "camera")]
            Pixels::Frame {
                planes,
                orientation,
            } => {
                match planes {
                    waterkit_camera::FramePlanes::Rgb(view) => {
                        let raster = read_raster(
                            view.texture(),
                            Texel::Direct,
                            *orientation,
                            context.device(),
                            context.queue(),
                        )
                        .await?;
                        bitmap_from_raster(&raster)?
                    }
                    // Luma alone is a sharper input for a text recognizer
                    // than a color conversion: no resampling artifacts, and
                    // the engine thresholds on intensity.
                    waterkit_camera::FramePlanes::YCbCr420 { luma, .. } => {
                        let raster = read_raster(
                            luma.texture(),
                            Texel::Direct,
                            *orientation,
                            context.device(),
                            context.queue(),
                        )
                        .await?;
                        bitmap_from_raster(&raster)?
                    }
                    waterkit_camera::FramePlanes::YCbCr422 { yuyv, .. } => {
                        let raster = read_raster(
                            yuyv.texture(),
                            Texel::LumaOfYuyv,
                            *orientation,
                            context.device(),
                            context.queue(),
                        )
                        .await?;
                        bitmap_from_raster(&raster)?
                    }
                }
            }
            Pixels::Encoded(bytes) => decode_encoded(bytes).await?,
        };
        Self::within_limits(bitmap)
    }
}

impl PreparedBitmap {
    fn within_limits(bitmap: SoftwareBitmap) -> Result<Self, VisionError> {
        let width = u32::try_from(
            bitmap
                .PixelWidth()
                .map_err(platform("SoftwareBitmap::PixelWidth"))?,
        )
        .map_err(|error| VisionError::Platform(format!("SoftwareBitmap::PixelWidth: {error}")))?;
        let height = u32::try_from(
            bitmap
                .PixelHeight()
                .map_err(platform("SoftwareBitmap::PixelHeight"))?,
        )
        .map_err(|error| VisionError::Platform(format!("SoftwareBitmap::PixelHeight: {error}")))?;
        let max =
            OcrEngine::MaxImageDimension().map_err(platform("OcrEngine::MaxImageDimension"))?;
        if width > max || height > max {
            return Err(VisionError::Unsupported(format!(
                "a {width}x{height} image exceeds Windows.Media.Ocr's {max} pixel limit"
            )));
        }
        Ok(Self {
            bitmap,
            width,
            height,
        })
    }
}

/// Wraps `bytes` in an `IBuffer` for the Imaging APIs.
fn ibuffer(bytes: &[u8]) -> Result<windows::Storage::Streams::IBuffer, VisionError> {
    let writer = DataWriter::new().map_err(platform("DataWriter::new"))?;
    writer
        .WriteBytes(bytes)
        .map_err(platform("DataWriter::WriteBytes"))?;
    writer
        .DetachBuffer()
        .map_err(platform("DataWriter::DetachBuffer"))
}

/// Decodes JPEG, PNG or HEIF bytes, honoring their orientation metadata.
async fn decode_encoded(bytes: &Bytes) -> Result<SoftwareBitmap, VisionError> {
    let stream =
        InMemoryRandomAccessStream::new().map_err(platform("InMemoryRandomAccessStream::new"))?;
    let write = {
        let buffer = ibuffer(bytes)?;
        stream
            .WriteAsync(&buffer)
            .map_err(platform("IOutputStream::WriteAsync"))?
    };
    write.await.map_err(platform("IOutputStream::WriteAsync"))?;
    stream
        .Seek(0)
        .map_err(platform("IRandomAccessStream::Seek"))?;
    let decoder = BitmapDecoder::CreateAsync(&stream)
        .map_err(platform("BitmapDecoder::CreateAsync"))?
        .await
        .map_err(platform("BitmapDecoder::CreateAsync"))?;
    let frame = decoder
        .GetFrameAsync(0)
        .map_err(platform("BitmapDecoder::GetFrameAsync"))?
        .await
        .map_err(platform("BitmapDecoder::GetFrameAsync"))?;
    frame
        .GetSoftwareBitmapTransformedAsync(
            BitmapPixelFormat::Bgra8,
            BitmapAlphaMode::Ignore,
            &BitmapTransform::new().map_err(platform("BitmapTransform::new"))?,
            ExifOrientationMode::RespectExifOrientation,
            ColorManagementMode::DoNotColorManage,
        )
        .map_err(platform("BitmapFrame::GetSoftwareBitmapTransformedAsync"))?
        .await
        .map_err(platform("BitmapFrame::GetSoftwareBitmapTransformedAsync"))
}

/// How texels become bitmap pixels.
#[derive(Debug, Clone, Copy)]
enum Texel {
    /// Each texel is one pixel; the texture format decides the bitmap
    /// format.
    Direct,
    /// Each 4-byte (Y0, Cb, Y1, Cr) texel is two pixels; luma only. The
    /// camera's `YCbCr422` contract guarantees `Rgba8Unorm` packing.
    #[cfg(feature = "camera")]
    LumaOfYuyv,
}

impl Texel {
    fn layout(self, format: wgpu::TextureFormat) -> Result<Layout, VisionError> {
        use wgpu::TextureFormat as F;
        let direct = |bpp: usize, format: BitmapPixelFormat| Layout {
            src_bytes: bpp,
            pixels_per_texel: 1,
            dst_bytes: bpp,
            format,
        };
        match (self, format) {
            (Self::Direct, F::Rgba8Unorm | F::Rgba8UnormSrgb) => {
                Ok(direct(4, BitmapPixelFormat::Rgba8))
            }
            (Self::Direct, F::Bgra8Unorm | F::Bgra8UnormSrgb) => {
                Ok(direct(4, BitmapPixelFormat::Bgra8))
            }
            (Self::Direct, F::R8Unorm) => Ok(direct(1, BitmapPixelFormat::Gray8)),
            (Self::Direct, F::R16Unorm) => Ok(direct(2, BitmapPixelFormat::Gray16)),
            #[cfg(feature = "camera")]
            (Self::LumaOfYuyv, F::Rgba8Unorm) => Ok(Layout {
                src_bytes: 2,
                pixels_per_texel: 2,
                dst_bytes: 1,
                format: BitmapPixelFormat::Gray8,
            }),
            (_, format) => Err(VisionError::Gpu(format!(
                "Windows.Media.Ocr cannot consume a {format:?} texture"
            ))),
        }
    }
}

/// How a readback maps into a `SoftwareBitmap`.
#[derive(Debug, Clone, Copy)]
struct Layout {
    /// Bytes per source pixel. For `LumaOfYuyv` this is 2: two pixels share a
    /// 4-byte group, so pixel `x` reads its luma at byte offset `x * 2`.
    src_bytes: usize,
    /// Logical pixels per source texel.
    pixels_per_texel: usize,
    /// Bytes per bitmap pixel.
    dst_bytes: usize,
    /// The `SoftwareBitmap` format to create.
    format: BitmapPixelFormat,
}

/// Upright pixels, ready for `SoftwareBitmap::CreateCopyFromBuffer`.
#[derive(Debug)]
struct Raster {
    data: Vec<u8>,
    width: u32,
    height: u32,
    layout: Layout,
}

/// Reads a texture back to the CPU and reshapes it upright.
///
/// The GPU→CPU copy is the unavoidable half of the texture path; its cost is
/// traced so callers can see what a frame costs them.
async fn read_raster(
    texture: &wgpu::Texture,
    texel: Texel,
    orientation: Orientation,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> Result<Raster, VisionError> {
    if !texture.usage().contains(wgpu::TextureUsages::COPY_SRC) {
        return Err(VisionError::Gpu(format!(
            "a {:?} texture lacks COPY_SRC usage for Windows.Media.Ocr",
            texture.format()
        )));
    }
    let layout = texel.layout(texture.format())?;
    let size = texture.size();
    let texel_width = size.width as usize;
    let texel_height = size.height as usize;
    let width = texel_width * layout.pixels_per_texel;
    let pitch =
        (width * layout.src_bytes).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("vision text readback"),
        size: (pitch * texel_height) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("vision text readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(
                    u32::try_from(pitch)
                        .map_err(|error| VisionError::Gpu(format!("readback pitch: {error}")))?,
                ),
                rows_per_image: Some(
                    u32::try_from(texel_height)
                        .map_err(|error| VisionError::Gpu(format!("readback height: {error}")))?,
                ),
            },
        },
        size,
    );
    queue.submit([encoder.finish()]);
    let started = Instant::now();
    map_read(device, buffer.slice(..)).await?;
    let raster = {
        let data = buffer
            .slice(..)
            .get_mapped_range()
            .map_err(|error| VisionError::Gpu(format!("texture mapped range: {error}")))?;
        upright(&data, pitch, width, texel_height, layout, orientation)
    };
    buffer.unmap();
    tracing::debug!(
        width,
        height = texel_height,
        elapsed = ?started.elapsed(),
        "texture readback for Windows.Media.Ocr"
    );
    Ok(raster)
}

/// Maps `slice` without blocking a thread: the device is polled between
/// await points until the map callback lands.
async fn map_read(device: &wgpu::Device, slice: wgpu::BufferSlice<'_>) -> Result<(), VisionError> {
    let (sender, mut receiver) = futures::channel::oneshot::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    loop {
        device
            .poll(wgpu::PollType::Poll)
            .map_err(|error| VisionError::Gpu(format!("wgpu poll: {error}")))?;
        match futures::poll!(&mut receiver) {
            Poll::Ready(result) => {
                return result
                    .expect("the map callback outlives this future")
                    .map_err(|error| VisionError::Gpu(format!("texture map: {error}")));
            }
            Poll::Pending => yield_once().await,
        }
    }
}

/// Yields to the executor once, without blocking or timing out.
async fn yield_once() {
    let mut yielded = false;
    poll_fn(|context| {
        if yielded {
            Poll::Ready(())
        } else {
            yielded = true;
            context.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await;
}

/// Reshapes a readback into upright bitmap pixels.
///
/// Iterates source rows so reads stay sequential; writes are strided on
/// quarter turns.
#[expect(
    clippy::cast_possible_truncation,
    reason = "image dimensions fit in u32 for any texture wgpu accepts"
)]
fn upright(
    source: &[u8],
    pitch: usize,
    width: usize,
    height: usize,
    layout: Layout,
    orientation: Orientation,
) -> Raster {
    let (out_width, out_height) = if orientation.swaps_dimensions() {
        (height, width)
    } else {
        (width, height)
    };
    let mut data = vec![0; out_width * out_height * layout.dst_bytes];
    for sy in 0..height {
        for sx in 0..width {
            let (dx, dy) = upright_position(sx, sy, width, height, orientation);
            let src = sy * pitch + sx * layout.src_bytes;
            let dst = (dy * out_width + dx) * layout.dst_bytes;
            data[dst..dst + layout.dst_bytes].copy_from_slice(&source[src..src + layout.dst_bytes]);
        }
    }
    Raster {
        data,
        width: out_width as u32,
        height: out_height as u32,
        layout,
    }
}

/// Where stored pixel `(sx, sy)` lands in the upright image.
const fn upright_position(
    sx: usize,
    sy: usize,
    width: usize,
    height: usize,
    orientation: Orientation,
) -> (usize, usize) {
    match orientation {
        Orientation::Up => (sx, sy),
        Orientation::UpMirrored => (width - 1 - sx, sy),
        Orientation::Down => (width - 1 - sx, height - 1 - sy),
        Orientation::DownMirrored => (sx, height - 1 - sy),
        Orientation::LeftMirrored => (sy, sx),
        Orientation::Right => (height - 1 - sy, sx),
        Orientation::RightMirrored => (height - 1 - sy, width - 1 - sx),
        Orientation::Left => (sy, width - 1 - sx),
    }
}

/// Builds a `SoftwareBitmap` from upright pixels.
fn bitmap_from_raster(raster: &Raster) -> Result<SoftwareBitmap, VisionError> {
    SoftwareBitmap::CreateCopyFromBuffer(
        &ibuffer(&raster.data)?,
        raster.layout.format,
        i32::try_from(raster.width)
            .map_err(|error| VisionError::Platform(format!("bitmap width: {error}")))?,
        i32::try_from(raster.height)
            .map_err(|error| VisionError::Platform(format!("bitmap height: {error}")))?,
    )
    .map_err(platform("SoftwareBitmap::CreateCopyFromBuffer"))
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
