//! The `vision` case: real `Vision` results against images generated here —
//! QR and Code 128 encoded at run time, and text rasterized with `DejaVu`.
//! No fixture is committed.

use std::sync::Arc;

use waterkit::vision::{
    DetectBarcodes, Image, Orientation, RecognitionLevel, RecognizeText, Symbology, Vision,
};
use waterkit_test_report::{TestCase, TestReport};

/// The `Vision` engine and its GPU handles, or the failure message.
async fn engine() -> Result<(Vision, Arc<wgpu::Device>, Arc<wgpu::Queue>), String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        })
        .await
        .map_err(|error| format!("no wgpu adapter on this device: {error}"))?;
    // The simulator's Metal supports fewer inter-stage shader variables
    // than wgpu's defaults ask for; take exactly what the adapter offers.
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            required_limits: adapter.limits(),
            ..Default::default()
        })
        .await
        .map_err(|error| format!("no wgpu device on this device: {error}"))?;
    let (device, queue) = (Arc::new(device), Arc::new(queue));
    Ok((Vision::new(device.clone(), queue.clone()), device, queue))
}

/// A QR code rendered to RGBA pixels.
fn qr_rgba(content: &str) -> (Vec<u8>, u32, u32) {
    let code = qrcode::QrCode::new(content.as_bytes()).expect("content fits a QR code");
    let luma = code
        .render::<image::Luma<u8>>()
        .min_dimensions(256, 256)
        .build();
    let rgba = image::DynamicImage::ImageLuma8(luma).to_rgba8();
    let (width, height) = (rgba.width(), rgba.height());
    (rgba.into_raw(), width, height)
}

/// A Code 128 barcode rendered to RGBA pixels.
fn code128_rgba(content: &str) -> (Vec<u8>, u32, u32) {
    const XDIM: usize = 3;
    const QUIET: usize = 12 * XDIM;
    const HEIGHT: usize = 150;

    // `Ɓ` selects barcoders' start set B; it is encoding syntax, not payload.
    let bars = barcoders::sym::code128::Code128::new(format!("\u{181}{content}"))
        .expect("content encodes as Code 128")
        .encode();
    let width = bars.len() * XDIM + 2 * QUIET;
    let mut pixels = vec![255u8; width * HEIGHT * 4];
    for x in QUIET..width - QUIET {
        if bars[(x - QUIET) / XDIM] == 1 {
            for y in 0..HEIGHT {
                let start = (y * width + x) * 4;
                pixels[start..start + 3].fill(0);
            }
        }
    }
    (
        pixels,
        u32::try_from(width).expect("the barcode fits the image"),
        u32::try_from(HEIGHT).expect("the barcode fits the image"),
    )
}

/// `line` rasterized with `DejaVu` Sans to RGBA pixels.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_lossless,
    clippy::cast_sign_loss
)]
fn text_rgba(line: &str, width: u32, height: u32) -> (Vec<u8>, u32, u32) {
    use ab_glyph::{Font, FontArc, PxScale, ScaleFont, point};

    let font = FontArc::try_from_slice(dejavu::sans::regular()).expect("DejaVu Sans parses");
    let scale = PxScale::from(56.0);
    let scaled = font.as_scaled(scale);
    let mut pixels = vec![255u8; (width * height * 4) as usize];
    let mut caret = point(24.0, 24.0 + scaled.ascent());
    let mut last = None;
    for ch in line.chars() {
        let id = scaled.glyph_id(ch);
        if let Some(previous) = last {
            caret.x += scaled.kern(previous, id);
        }
        let glyph = id.with_scale_and_position(scale, caret);
        if let Some(outline) = font.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            outline.draw(|dx, dy, coverage| {
                let x = bounds.min.x as i64 + i64::from(dx);
                let y = bounds.min.y as i64 + i64::from(dy);
                if (0..width as i64).contains(&x) && (0..height as i64).contains(&y) {
                    let shade = 255 - (coverage * 255.0) as u8;
                    let start = ((y as u32 * width + x as u32) * 4) as usize;
                    pixels[start..start + 4].copy_from_slice(&[shade, shade, shade, 255]);
                }
            });
        }
        caret.x += scaled.h_advance(id);
        last = Some(id);
    }
    (pixels, width, height)
}

/// RGBA pixels uploaded into a `TEXTURE_BINDING` texture, then submitted.
fn textured(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    (pixels, width, height): (Vec<u8>, u32, u32),
    orientation: Orientation,
) -> Image {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("vision case image"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    // Staged writes must reach the texture before Vision samples it.
    queue.submit(std::iter::empty());
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    Image::from_texture(texture, orientation)
}

/// The same pixels as PNG-encoded still image data.
fn encoded((pixels, width, height): (Vec<u8>, u32, u32)) -> Image {
    let rgba = image::RgbaImage::from_raw(width, height, pixels).expect("rgba buffer is sized");
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut png, image::ImageFormat::Png)
        .expect("png encoding succeeds");
    Image::from_encoded(bytes::Bytes::from(png.into_inner()))
}

/// `expect` on a `Result`, then assertions on its value as a `TestCase`.
fn check(name: &str, result: Result<(), String>) -> TestCase {
    match result {
        Ok(()) => TestCase::passed(name),
        Err(message) => TestCase::failed(name, message),
    }
}

/// A barcode-decode case. Barcode detection reports its full
/// `supportedSymbologies` set on the simulator but decodes nothing there:
/// its detector model cannot run under the simulator's CPU-only ML
/// runtime. An empty result there reports as skipped; a device decodes
/// for real.
async fn barcode_decode(
    vision: &Vision,
    name: &str,
    image: Image,
    symbology: Symbology,
    payload: &str,
) -> TestCase {
    match vision
        .perform(&image, &DetectBarcodes::new(symbology))
        .await
    {
        Err(error) => TestCase::failed(name, format!("request failed: {error}")),
        Ok(barcodes) if barcodes.is_empty() && cfg!(target_abi = "sim") => TestCase::skipped(
            name,
            "barcode detection returns no observations on the simulator; verify on a device",
        ),
        Ok(barcodes) => match barcodes.as_slice() {
            [barcode]
                if barcode.symbology() == symbology
                    && barcode.payload().text() == Some(payload) =>
            {
                TestCase::passed_with_message(name, format!("payload={payload}"))
            }
            barcodes => TestCase::failed(name, format!("decoded {barcodes:?}")),
        },
    }
}

pub async fn record(report: &mut TestReport) {
    let (vision, device, queue) = match engine().await {
        Ok(engine) => engine,
        Err(error) => {
            report.push(TestCase::failed("vision.engine", error));
            return;
        }
    };

    let capabilities = vision.capabilities();
    if !capabilities.barcodes.native.is_empty()
        && capabilities.barcodes.native.contains(Symbology::Qr)
        && capabilities.barcodes.native.contains(Symbology::Code128)
        && !capabilities.text.native.is_empty()
    {
        report.push(TestCase::passed_with_message(
            "vision.capabilities",
            format!(
                "symbologies={} languages={}",
                capabilities.barcodes.native.len(),
                capabilities.text.native.len()
            ),
        ));
    } else {
        report.push(TestCase::failed(
            "vision.capabilities",
            format!("Vision reports {capabilities:?}"),
        ));
    }

    report.push(
        barcode_decode(
            &vision,
            "vision.qr_encoded",
            encoded(qr_rgba("waterkit-ios")),
            Symbology::Qr,
            "waterkit-ios",
        )
        .await,
    );
    report.push(
        barcode_decode(
            &vision,
            "vision.barcodes_texture",
            textured(
                &device,
                &queue,
                code128_rgba("WATERKIT-IOS"),
                Orientation::Up,
            ),
            Symbology::Code128,
            "WATERKIT-IOS",
        )
        .await,
    );

    report.push(check(
        "vision.text",
        (vision
            .perform(
                &encoded(text_rgba("waterkit sees", 720, 140)),
                &RecognizeText::new().level(RecognitionLevel::Accurate),
            )
            .await)
            .map_err(|error| format!("text request failed: {error}"))
            .and_then(|lines| match lines.as_slice() {
                [line] if line.text.to_lowercase() == "waterkit sees" => Ok(()),
                lines => Err(format!("recognized {lines:?}")),
            }),
    ));

    report.push(
        match vision
            .perform(
                &encoded(qr_rgba("x")),
                &RecognizeText::new().languages(["tlh".parse().expect("tlh is BCP-47")]),
            )
            .await
        {
            Err(waterkit::vision::VisionError::Unsupported(message)) if message.contains("tlh") => {
                TestCase::passed("vision.unsupported_language")
            }
            Err(error) => TestCase::failed(
                "vision.unsupported_language",
                format!("expected Unsupported naming tlh, got {error}"),
            ),
            Ok(_) => TestCase::failed("vision.unsupported_language", "a tlh request was served"),
        },
    );
}
