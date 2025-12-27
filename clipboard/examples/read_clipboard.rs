//! Clipboard reading demo.
#![allow(clippy::cast_possible_truncation, clippy::ignored_unit_patterns)]
fn main() {
    println!("Reading clipboard...");
    match waterkit_clipboard::get_text() {
        Some(text) => println!("Clipboard text content:\n{text}"),
        None => println!("Clipboard does not contain text."),
    }

    match waterkit_clipboard::get_image() {
        Some(image) => {
            println!(
                "Clipboard contains image: {}x{} ({} bytes)",
                image.width,
                image.height,
                image.bytes.len()
            );

            // Save to file for preview
            match image::save_buffer(
                "clipboard_preview.png",
                &image.bytes,
                image.width as u32,
                image.height as u32,
                image::ColorType::Rgba8,
            ) {
                Ok(_) => println!("Image saved to clipboard_preview.png"),
                Err(e) => println!("Failed to save image: {e}"),
            }
        }
        None => println!("Clipboard does not contain image."),
    }
}
