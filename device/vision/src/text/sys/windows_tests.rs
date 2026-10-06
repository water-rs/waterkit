//! Tests for the `Windows.Media.Ocr` text realization. Text fixtures are
//! rendered at test time with the OS's Arial; no binary fixtures are
//! committed.

use std::time::Instant;

use ab_glyph::{Font, ScaleFont};
use bytes::Bytes;
use icu_locale_core::langid;
use image::ImageEncoder;

use super::*;
use crate::{
    Image, Vision,
    capability::Portable,
    test_support::gpu,
    text::{RecognitionLevel, RecognizeText},
};

/// Rasterizes `lines` of text with the OS's Arial into a white grayscale
/// image.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "test glyph metrics are small non-negative f32 values"
)]
fn render(lines: &[&str]) -> (Vec<u8>, u32, u32) {
    const PAD: usize = 24;
    let font = ab_glyph::FontArc::try_from_vec(
        std::fs::read(r"C:\Windows\Fonts\arial.ttf").expect("Arial ships with Windows"),
    )
    .expect("Arial parses as a font");
    let scale = ab_glyph::PxScale::from(72.0);
    let scaled = font.as_scaled(scale);
    let line_height = (scaled.ascent() - scaled.descent() + scaled.line_gap()).ceil() as usize + 16;
    let width = lines
        .iter()
        .map(|line| {
            line.chars()
                .map(|c| scaled.h_advance(scaled.glyph_id(c)))
                .sum::<f32>()
        })
        .fold(0.0_f32, f32::max)
        .ceil() as usize
        + 2 * PAD;
    let height = lines.len() * line_height + 2 * PAD;
    let mut gray = vec![255u8; width * height];
    for (row, line) in lines.iter().enumerate() {
        let mut caret = PAD as f32;
        let baseline = (row as f32).mul_add(line_height as f32, PAD as f32) + scaled.ascent();
        for ch in line.chars() {
            let id = scaled.glyph_id(ch);
            let glyph = id.with_scale_and_position(scale, ab_glyph::point(caret, baseline));
            caret += scaled.h_advance(id);
            if let Some(outlined) = scaled.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                outlined.draw(|x, y, coverage| {
                    let px = bounds.min.x as usize + x as usize;
                    let py = bounds.min.y as usize + y as usize;
                    if px < width && py < height {
                        let dst = &mut gray[py * width + px];
                        *dst = dst.saturating_sub((coverage * 255.0) as u8);
                    }
                });
            }
        }
    }
    (
        gray,
        u32::try_from(width).expect("fixture width fits u32"),
        u32::try_from(height).expect("fixture height fits u32"),
    )
}

fn bgra(gray: &[u8]) -> Vec<u8> {
    gray.iter().flat_map(|v| [*v, *v, *v, 255]).collect()
}

const LINES: [&str; 3] = [
    "THE QUICK BROWN FOX",
    "JUMPS OVER LAZY DOG",
    "PACK MY BOX 0123456789",
];

fn texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("vision text test"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

fn write(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    data: &[u8],
    bytes_per_row: u32,
    width: u32,
    height: u32,
) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(bytes_per_row),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

fn bgra_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    bgra: &[u8],
    width: u32,
    height: u32,
) -> wgpu::Texture {
    let texture = texture(device, width, height, wgpu::TextureFormat::Bgra8Unorm);
    write(queue, &texture, bgra, width * 4, width, height);
    texture
}

/// A vision engine, or `None` when no English recognizer is installed — the
/// end-to-end tests then skip rather than fake the capability.
fn english_vision() -> Option<Vision> {
    let (device, queue) = gpu();
    let vision = Vision::new(device, queue);
    let has_english = vision
        .capabilities()
        .text
        .native
        .iter()
        .any(|id| id.to_string().starts_with("en"));
    if has_english {
        Some(vision)
    } else {
        tracing::warn!("no English Windows.Media.Ocr recognizer; skipping test");
        None
    }
}

/// Rotates BGRA pixels 90° counter-clockwise, producing what a camera would
/// store for `Orientation::Right` (upright = stored rotated clockwise).
fn store_rotated_ccw(bgra: &[u8], width: u32, height: u32) -> (Vec<u8>, u32, u32) {
    let w = width as usize;
    let h = height as usize;
    let (sw, sh) = (h, w);
    let mut out = vec![0u8; bgra.len()];
    for y in 0..sh {
        for x in 0..sw {
            let (ux, uy) = (w - 1 - y, x);
            out[(y * sw + x) * 4..(y * sw + x) * 4 + 4]
                .copy_from_slice(&bgra[(uy * w + ux) * 4..(uy * w + ux) * 4 + 4]);
        }
    }
    (out, u32::try_from(sw).unwrap(), u32::try_from(sh).unwrap())
}

/// Asserts the rendered text was read and every box is a normalized upright
/// `Quad`.
fn assert_read(lines: &[TextLine], expected: &[&str]) {
    assert!(lines.len() >= 2, "expected several lines, got {lines:?}");
    let text = lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    for word in expected {
        assert!(text.contains(word), "OCR read {text:?}, expected {word:?}");
    }
    for line in lines {
        assert!(!line.text.is_empty() && !line.words.is_empty());
        for point in line.bounds.0 {
            assert!(
                (0.0..=1.0).contains(&point.x) && (0.0..=1.0).contains(&point.y),
                "quad {point:?} leaves the normalized image"
            );
        }
        for word in &line.words {
            assert_ne!(word.text, "");
            for point in word.bounds.0 {
                assert!(
                    (0.0..=1.0).contains(&point.x) && (0.0..=1.0).contains(&point.y),
                    "word quad {point:?} leaves the normalized image"
                );
            }
        }
    }
}

#[test]
fn capabilities_list_the_os_ocr_languages() {
    let (device, queue) = gpu();
    let vision = Vision::new(device, queue);
    let capabilities = vision.capabilities();
    assert_eq!(capabilities.text.native, recognizer_languages());
    assert!(matches!(capabilities.text.portable, Portable::Absent));
}

#[test]
fn recognizes_text_in_a_texture() {
    let Some(vision) = english_vision() else {
        return;
    };
    let (gray, w, h) = render(&LINES);
    let texture = bgra_texture(&vision.device, &vision.queue, &bgra(&gray), w, h);
    let image = Image::from_texture(texture, Orientation::Up);
    let lines = pollster::block_on(
        vision.perform(&image, &RecognizeText::new().level(RecognitionLevel::Fast)),
    )
    .expect("OCR on a rendered text texture");
    assert_read(&lines, &["QUICK", "FOX", "JUMPS", "BOX"]);
}

#[test]
fn recognizes_text_in_an_encoded_png() {
    let Some(vision) = english_vision() else {
        return;
    };
    let (gray, w, h) = render(&LINES);
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&gray, w, h, image::ExtendedColorType::L8)
        .expect("test PNG encodes");
    let image = Image::from_encoded(Bytes::from(png));
    let lines = pollster::block_on(vision.perform(&image, &RecognizeText::new()))
        .expect("OCR on an encoded PNG");
    assert_read(&lines, &["QUICK", "FOX", "JUMPS", "BOX"]);
}

#[test]
fn uprights_a_rotated_texture_before_recognition() {
    let Some(vision) = english_vision() else {
        return;
    };
    let (gray, w, h) = render(&LINES);
    let (stored, sw, sh) = store_rotated_ccw(&bgra(&gray), w, h);
    let texture = bgra_texture(&vision.device, &vision.queue, &stored, sw, sh);
    let image = Image::from_texture(texture, Orientation::Right);
    let lines = pollster::block_on(vision.perform(&image, &RecognizeText::new()))
        .expect("OCR on a Right-oriented texture");
    assert_read(&lines, &["QUICK", "FOX", "JUMPS", "BOX"]);
}

#[cfg(feature = "camera")]
#[test]
fn recognizes_text_in_camera_planes() {
    use waterkit_camera::{FramePlanes, YcbcrEncoding, YcbcrMatrix, YcbcrRange};

    let Some(vision) = english_vision() else {
        return;
    };
    let encoding = YcbcrEncoding {
        matrix: YcbcrMatrix::Bt709,
        range: YcbcrRange::Video,
    };
    let (gray, w, h) = render(&LINES);
    let request = RecognizeText::new();

    let rgb_texture = bgra_texture(&vision.device, &vision.queue, &bgra(&gray), w, h);
    let rgb = FramePlanes::Rgb(rgb_texture.create_view(&wgpu::TextureViewDescriptor::default()));
    let lines =
        pollster::block_on(vision.perform(&Image::from_planes(&rgb, Orientation::Up), &request))
            .expect("OCR on RGB camera planes");
    assert_read(&lines, &["QUICK", "FOX"]);

    let luma_texture = texture(&vision.device, w, h, wgpu::TextureFormat::R8Unorm);
    write(&vision.queue, &luma_texture, &gray, w, w, h);
    let luma = luma_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let chroma_texture = texture(
        &vision.device,
        w.div_ceil(2),
        h.div_ceil(2),
        wgpu::TextureFormat::Rg8Unorm,
    );
    let ycbcr420 = FramePlanes::YCbCr420 {
        luma,
        chroma: chroma_texture.create_view(&wgpu::TextureViewDescriptor::default()),
        encoding,
    };
    let lines = pollster::block_on(
        vision.perform(&Image::from_planes(&ycbcr420, Orientation::Up), &request),
    )
    .expect("OCR on YCbCr420 camera planes");
    assert_read(&lines, &["QUICK", "FOX"]);

    let (w, h) = (w as usize, h as usize);
    let tex_w = w / 2;
    let mut yuyv = Vec::with_capacity(tex_w * h * 4);
    for y in 0..h {
        for x in 0..tex_w {
            yuyv.extend_from_slice(&[gray[y * w + 2 * x], 128, gray[y * w + 2 * x + 1], 128]);
        }
    }
    let tex_w = u32::try_from(tex_w).unwrap();
    let height = u32::try_from(h).unwrap();
    let yuyv_texture = texture(
        &vision.device,
        tex_w,
        height,
        wgpu::TextureFormat::Rgba8Unorm,
    );
    write(
        &vision.queue,
        &yuyv_texture,
        &yuyv,
        tex_w * 4,
        tex_w,
        height,
    );
    let ycbcr422 = FramePlanes::YCbCr422 {
        yuyv: yuyv_texture.create_view(&wgpu::TextureViewDescriptor::default()),
        encoding,
    };
    let lines = pollster::block_on(
        vision.perform(&Image::from_planes(&ycbcr422, Orientation::Up), &request),
    )
    .expect("OCR on YCbCr422 camera planes");
    assert_read(&lines, &["QUICK", "FOX"]);
}

#[test]
fn an_absent_language_is_unsupported_not_retried() {
    let (device, queue) = gpu();
    let vision = Vision::new(device, queue);
    let request = RecognizeText::new().languages([langid!("tlh")]);
    let error = pollster::block_on(vision.prepare(&request)).expect_err("Klingon is not installed");
    assert!(
        matches!(&error, VisionError::Unsupported(message) if message.contains("text") && message.contains("tlh")),
        "{error}"
    );
}

#[test]
fn several_languages_are_declined_at_selection() {
    let (device, queue) = gpu();
    let vision = Vision::new(device, queue);
    let request = RecognizeText::new().languages([langid!("en-US"), langid!("fr-FR")]);
    let error = pollster::block_on(vision.prepare(&request))
        .expect_err("one OcrEngine recognizes one language");
    assert!(
        matches!(&error, VisionError::Unsupported(message) if message.contains("en-US")),
        "{error}"
    );
}

/// Reports the GPU→CPU copy the texture path pays; the assertion keeps the
/// readback honest, not fast.
#[test]
fn texture_readback_cost_is_measured() {
    let (device, queue) = gpu();
    let (gray, w, h) = render(&LINES);
    let texture = bgra_texture(&device, &queue, &bgra(&gray), w, h);
    let mut elapsed = Vec::new();
    for _ in 0..3 {
        let started = Instant::now();
        let raster = pollster::block_on(read_raster(
            &texture,
            Texel::Direct,
            Orientation::Up,
            &device,
            &queue,
        ))
        .expect("readback");
        elapsed.push(started.elapsed());
        assert_eq!((raster.width, raster.height), (w, h));
    }
    eprintln!("read_raster {w}x{h} BGRA8: {elapsed:?}");
}
