//! Real `Vision` results against images generated at test time; no binary
//! fixtures are committed. The macOS run verifies the same bridge code iOS
//! ships.
#![cfg(all(target_os = "macos", feature = "barcode", feature = "text"))]

use std::sync::Arc;

use bytes::Bytes;
use enumset::EnumSet;
use image::{DynamicImage, Rgba, RgbaImage};
use waterkit_core::Capabilities;
#[cfg(feature = "document")]
use waterkit_vision::{Block, RecognizeDocument};
use waterkit_vision::{
    DetectBarcodes, Image, Orientation, RecognitionLevel, RecognizeText, Symbology, Vision,
    VisionError,
};

fn gpu() -> (Arc<wgpu::Device>, Arc<wgpu::Queue>) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .expect("vision tests need an adapter");
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .expect("vision tests need a device");
    (Arc::new(device), Arc::new(queue))
}

fn vision() -> (Vision, Arc<wgpu::Device>, Arc<wgpu::Queue>) {
    let (device, queue) = gpu();
    (Vision::new(device.clone(), queue.clone()), device, queue)
}

fn qr_rgba(content: &str) -> RgbaImage {
    let code = qrcode::QrCode::new(content.as_bytes()).expect("content fits a QR code");
    let luma = code
        .render::<image::Luma<u8>>()
        .min_dimensions(256, 256)
        .build();
    DynamicImage::ImageLuma8(luma).to_rgba8()
}

#[allow(clippy::cast_possible_truncation)] // generated barcodes fit u32
fn code128_rgba(content: &str) -> RgbaImage {
    const XDIM: u32 = 3;
    const QUIET: u32 = 12 * XDIM;

    // barcoders takes an explicit start-set marker; `Ɓ` selects set B. The
    // marker is encoding syntax, not part of the decoded payload.
    let bars = barcoders::sym::code128::Code128::new(format!("\u{181}{content}"))
        .expect("content encodes as Code 128")
        .encode();
    let width = bars.len() as u32 * XDIM + 2 * QUIET;
    RgbaImage::from_fn(width, 150, |x, _| {
        let index = x.saturating_sub(QUIET) / XDIM;
        let black = x >= QUIET && bars.get(index as usize) == Some(&1);
        if black {
            Rgba([0, 0, 0, 255])
        } else {
            Rgba([255, 255, 255, 255])
        }
    })
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_lossless,
    clippy::cast_sign_loss
)]
// glyph bounds are whole-pixel values; canvas sizes and coverage are small
fn draw_text(image: &mut RgbaImage, text: &str, origin: (f32, f32), size: f32) {
    use ab_glyph::{Font, FontArc, PxScale, ScaleFont, point};

    let font = FontArc::try_from_slice(dejavu::sans::regular()).expect("DejaVu Sans parses");
    let scale = PxScale::from(size);
    let scaled = font.as_scaled(scale);
    let (width, height) = (i64::from(image.width()), i64::from(image.height()));
    let mut caret = point(origin.0, origin.1 + scaled.ascent());
    let mut last = None;
    for ch in text.chars() {
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
                if (0..width).contains(&x) && (0..height).contains(&y) {
                    let shade = 255 - (coverage * 255.0) as u8;
                    *image.get_pixel_mut(x as u32, y as u32) = Rgba([shade, shade, shade, 255]);
                }
            });
        }
        caret.x += scaled.h_advance(id);
        last = Some(id);
    }
}

fn text_rgba(line: &str, width: u32, height: u32) -> RgbaImage {
    let mut image = RgbaImage::from_pixel(width, height, Rgba([255, 255, 255, 255]));
    draw_text(&mut image, line, (24.0, 24.0), 56.0);
    image
}

fn encoded(image: &RgbaImage) -> Image {
    let mut png = std::io::Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image.clone())
        .write_to(&mut png, image::ImageFormat::Png)
        .expect("png encoding succeeds");
    Image::from_encoded(Bytes::from(png.into_inner()))
}

fn textured(
    image: &RgbaImage,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    orientation: Orientation,
) -> Image {
    let (width, height) = (image.width(), image.height());
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("vision test image"),
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
        image.as_raw(),
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
    // `write_texture` is staged in wgpu's pending-writes encoder: submit it
    // and wait, so Vision reads finished pixels, as a rendered frame would.
    queue.submit(std::iter::empty());
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("the upload completes");
    Image::from_texture(texture, orientation)
}

#[test]
fn capabilities_report_the_running_oss_vision_support() {
    let (vision, _, _) = vision();
    let capabilities = vision.capabilities();
    assert!(capabilities.available());
    for symbology in [
        Symbology::Qr,
        Symbology::Code128,
        Symbology::Ean13,
        Symbology::Pdf417,
        Symbology::Aztec,
        Symbology::DataMatrix,
    ] {
        assert!(
            capabilities.barcodes.native.contains(symbology),
            "Vision serves {symbology:?} on every macOS 26"
        );
    }
    assert!(
        !capabilities.text.native.is_empty(),
        "Vision serves text languages on every macOS 26"
    );
}

#[test]
fn qr_in_an_encoded_png_decodes_with_payload_and_bounds() {
    let (vision, _, _) = vision();
    let request = DetectBarcodes::new(EnumSet::only(Symbology::Qr));
    let barcodes = pollster::block_on(vision.perform(&encoded(&qr_rgba("waterkit")), &request))
        .expect("Vision decodes the generated QR");

    assert_eq!(barcodes.len(), 1);
    let barcode = &barcodes[0];
    assert_eq!(barcode.symbology(), Symbology::Qr);
    assert_eq!(barcode.payload().text(), Some("waterkit"));
    let [top_left, top_right, _, bottom_left] = barcode.bounds().0;
    for corner in [top_left, top_right, bottom_left] {
        assert!((0.0..=1.0).contains(&corner.x) && (0.0..=1.0).contains(&corner.y));
    }
    assert!(top_left.y < bottom_left.y);
    assert!(top_left.x < top_right.x);
}

#[test]
fn qr_and_code128_decode_from_gpu_textures() {
    let (vision, device, queue) = vision();
    let request = DetectBarcodes::new(EnumSet::only(Symbology::Qr) | Symbology::Code128);

    let barcodes = pollster::block_on(vision.perform(
        &textured(&qr_rgba("from-gpu"), &device, &queue, Orientation::Up),
        &request,
    ))
    .expect("QR decodes from an MTLTexture");
    assert_eq!(barcodes.len(), 1);
    assert_eq!(barcodes[0].symbology(), Symbology::Qr);
    assert_eq!(barcodes[0].payload().text(), Some("from-gpu"));

    let barcodes = pollster::block_on(vision.perform(
        &textured(&code128_rgba("WATERKIT"), &device, &queue, Orientation::Up),
        &request,
    ))
    .expect("Code 128 decodes from an MTLTexture");
    assert_eq!(barcodes.len(), 1);
    assert_eq!(barcodes[0].symbology(), Symbology::Code128);
    assert_eq!(barcodes[0].payload().text(), Some("WATERKIT"));
}

#[test]
fn stored_orientation_is_applied_before_vision_reads() {
    let (vision, device, queue) = vision();
    // `Right` marks pixels stored turned 90° counter-clockwise from upright.
    let rotated = image::imageops::rotate270(&qr_rgba("oriented"));
    let request = DetectBarcodes::new(EnumSet::only(Symbology::Qr));
    let barcodes = pollster::block_on(vision.perform(
        &textured(&rotated, &device, &queue, Orientation::Right),
        &request,
    ))
    .expect("the rotated texture still decodes");
    assert_eq!(barcodes.len(), 1);
    assert_eq!(barcodes[0].payload().text(), Some("oriented"));
}

#[test]
fn rendered_text_is_recognized() {
    let (vision, _, _) = vision();
    let request = RecognizeText::new().level(RecognitionLevel::Accurate);
    let lines = pollster::block_on(
        vision.perform(&encoded(&text_rgba("waterkit sees", 720, 140)), &request),
    )
    .expect("Vision recognizes rendered text");

    assert_eq!(lines.len(), 1);
    let line = &lines[0];
    assert_eq!(line.text.to_lowercase(), "waterkit sees");
    assert!(line.confidence.is_some_and(|confidence| confidence > 0.5));

    // `boundingBox(for:)` splits the line into words with real bounds.
    let words: Vec<&str> = line.words.iter().map(|word| word.text.as_str()).collect();
    assert_eq!(words, ["waterkit", "sees"]);
    for word in &line.words {
        assert!(word.confidence.is_some());
        let [tl, _, br, _] = word.bounds.0;
        assert!(br.x > tl.x && br.y > tl.y, "word {word:?} has empty bounds");
    }
}

#[test]
fn a_request_tuple_shares_one_image_and_reports_each_result() {
    let (vision, _, _) = vision();
    // A QR and a rendered line composed onto one canvas.
    let mut canvas = RgbaImage::from_pixel(720, 560, Rgba([255, 255, 255, 255]));
    image::imageops::overlay(&mut canvas, &qr_rgba("shared pass"), 24, 24);
    image::imageops::overlay(&mut canvas, &text_rgba("shared pass", 680, 100), 20, 420);

    let request = (
        DetectBarcodes::new(EnumSet::only(Symbology::Qr)),
        RecognizeText::new(),
    );
    let (barcodes, lines) = pollster::block_on(vision.perform(&encoded(&canvas), &request))
        .expect("both requests share the pass");

    assert_eq!(barcodes.len(), 1);
    assert_eq!(barcodes[0].payload().text(), Some("shared pass"));
    assert!(
        lines
            .iter()
            .any(|line| line.text.to_lowercase().contains("shared")),
        "recognized lines: {lines:?}"
    );
}

#[test]
fn an_unserved_language_fails_ahead_of_time_and_names_it() {
    let (vision, _, _) = vision();
    // BCP-47 "tlh" (Klingon) parses but Vision does not serve it.
    let request = RecognizeText::new().languages(["tlh".parse().expect("tlh is BCP-47")]);
    let error = pollster::block_on(vision.perform(&encoded(&text_rgba("x", 200, 80)), &request))
        .expect_err("an unsupported language is rejected before any native call");
    let VisionError::Unsupported(message) = error else {
        panic!("expected Unsupported, got {error:?}")
    };
    assert!(
        message.contains("tlh"),
        "the error names the language: {message}"
    );
}

#[cfg(feature = "document")]
fn fill_rect(image: &mut RgbaImage, x: u32, y: u32, width: u32, height: u32) {
    for row in 0..height {
        for column in 0..width {
            image.put_pixel(x + column, y + row, Rgba([0, 0, 0, 255]));
        }
    }
}

/// A page holding two paragraphs, a bordered 2x2 table and a numbered
/// two-item list, rendered at test time.
#[cfg(feature = "document")]
fn document_page_rgba() -> RgbaImage {
    let mut page = RgbaImage::from_pixel(1000, 1400, Rgba([255, 255, 255, 255]));
    draw_text(
        &mut page,
        "The quarterly report summarizes revenue",
        (60.0, 80.0),
        36.0,
    );
    draw_text(
        &mut page,
        "across all regions for the period.",
        (60.0, 140.0),
        36.0,
    );
    draw_text(
        &mut page,
        "Contact the sales office for further",
        (60.0, 260.0),
        36.0,
    );
    draw_text(
        &mut page,
        "details before the end of the month.",
        (60.0, 320.0),
        36.0,
    );

    // A bordered 2x2 grid: outer frame, one horizontal and one vertical
    // divider, with text in each cell.
    let (left, top, bottom) = (60, 460, 700);
    let (middle_x, middle_y) = (500, 580);
    fill_rect(&mut page, left, top, 880, 3);
    fill_rect(&mut page, left, middle_y, 880, 3);
    fill_rect(&mut page, left, bottom, 880, 3);
    fill_rect(&mut page, left, top, 3, bottom - top);
    fill_rect(&mut page, middle_x, top, 3, bottom - top);
    fill_rect(&mut page, left + 877, top, 3, bottom - top);
    draw_text(&mut page, "Region", (90.0, 490.0), 30.0);
    draw_text(&mut page, "Total", (530.0, 490.0), 30.0);
    draw_text(&mut page, "North", (90.0, 610.0), 30.0);
    draw_text(&mut page, "42", (530.0, 610.0), 30.0);

    draw_text(
        &mut page,
        "1. First item of the summary",
        (60.0, 780.0),
        36.0,
    );
    draw_text(
        &mut page,
        "2. Second item of the summary",
        (60.0, 840.0),
        36.0,
    );
    page
}

/// A page with two columns of two paragraphs each.
#[cfg(feature = "document")]
fn two_column_page_rgba() -> RgbaImage {
    let mut page = RgbaImage::from_pixel(1000, 1400, Rgba([255, 255, 255, 255]));
    draw_text(&mut page, "Left column opens the page", (60.0, 100.0), 34.0);
    draw_text(
        &mut page,
        "with a first paragraph here.",
        (60.0, 155.0),
        34.0,
    );
    draw_text(&mut page, "A second paragraph follows", (60.0, 300.0), 34.0);
    draw_text(
        &mut page,
        "below it in the left column.",
        (60.0, 355.0),
        34.0,
    );
    draw_text(
        &mut page,
        "Right column starts with its",
        (560.0, 100.0),
        34.0,
    );
    draw_text(
        &mut page,
        "own first paragraph on top.",
        (560.0, 155.0),
        34.0,
    );
    draw_text(
        &mut page,
        "And a second paragraph sits",
        (560.0, 300.0),
        34.0,
    );
    draw_text(
        &mut page,
        "below in the right column.",
        (560.0, 355.0),
        34.0,
    );
    page
}

#[test]
#[cfg(feature = "document")]
fn a_rendered_page_recognizes_its_document_structure() {
    let (vision, _, _) = vision();
    let document = pollster::block_on(
        vision.perform(&encoded(&document_page_rgba()), &RecognizeDocument::new()),
    )
    .expect("Vision recognizes the page's structure");

    let kinds: Vec<&str> = document
        .blocks
        .iter()
        .map(|block| match block {
            Block::Paragraph(_) => "paragraph",
            Block::Table(_) => "table",
            Block::List(_) => "list",
            Block::Barcode(_) => "barcode",
            Block::Formula(_) => "formula",
        })
        .collect();
    assert_eq!(
        kinds,
        ["paragraph", "paragraph", "table", "list"],
        "blocks in reading order: {document:#?}"
    );

    let [Block::Paragraph(first), Block::Paragraph(second), ..] = &document.blocks[..] else {
        unreachable!("the kinds assertion above orders paragraphs first")
    };
    assert!(first.text.contains("quarterly report"), "{first:?}");
    assert!(second.text.contains("sales office"), "{second:?}");

    let Some(Block::Table(table)) = document.blocks.get(2) else {
        unreachable!()
    };
    assert_eq!((table.rows, table.columns), (2, 2));
    assert_eq!(table.cells.len(), 4);
    for cell in &table.cells {
        assert_eq!(cell.rows.end - cell.rows.start, 1);
        assert_eq!(cell.columns.end - cell.columns.start, 1);
        assert!(
            matches!(cell.content[..], [Block::Paragraph(_)]),
            "cell {cell:?} holds one paragraph"
        );
    }

    let Some(Block::List(list)) = document.blocks.get(3) else {
        unreachable!()
    };
    assert_eq!(list.items.len(), 2);
    let items: Vec<String> = list
        .items
        .iter()
        .map(|item| {
            item.content
                .iter()
                .filter_map(|block| match block {
                    Block::Paragraph(paragraph) => Some(paragraph.text.as_str()),
                    _ => None,
                })
                .collect()
        })
        .collect();
    assert!(
        items[0].contains("First") && items[1].contains("Second"),
        "list items: {items:?}"
    );
}

#[test]
#[cfg(feature = "document")]
fn a_two_column_page_reads_left_column_then_right() {
    let (vision, _, _) = vision();
    let document = pollster::block_on(
        vision.perform(&encoded(&two_column_page_rgba()), &RecognizeDocument::new()),
    )
    .expect("Vision recognizes the page's structure");

    let paragraphs: Vec<&str> = document
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Paragraph(paragraph) => Some(paragraph.text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        paragraphs.len(),
        4,
        "four paragraphs in column order: {document:#?}"
    );
    let columns: Vec<&str> = paragraphs
        .iter()
        .map(|text| {
            let text = text.to_lowercase();
            if text.contains("left") {
                "left"
            } else if text.contains("right") {
                "right"
            } else {
                panic!("paragraph names its column: {text}")
            }
        })
        .collect();
    assert_eq!(columns, ["left", "left", "right", "right"]);
}

#[test]
fn a_request_symbology_set_is_checked_against_the_supported_set() {
    let (vision, _, _) = vision();
    // Everything the crate vocabulary names and Vision does not serve must
    // already be filtered out by `capabilities()`; the served subset decodes.
    let served = vision.capabilities().barcodes.native;
    assert!(served.contains(Symbology::Qr));
    let request = DetectBarcodes::new(EnumSet::only(Symbology::Qr));
    pollster::block_on(vision.prepare(&request)).expect("a served request prepares");
}
