//! Clipboard reading demo.

fn main() -> Result<(), waterkit_clipboard::ClipboardError> {
    futures::executor::block_on(read())
}

#[cfg_attr(
    target_arch = "wasm32",
    expect(
        clippy::future_not_send,
        reason = "awaits `Clipboard::text`, which in a browser is bound to its thread"
    )
)]
async fn read() -> Result<(), waterkit_clipboard::ClipboardError> {
    println!("Reading clipboard...\n");

    let clipboard = waterkit_clipboard::Clipboard::new()?;

    // Show available types
    let has_text = clipboard.has_text()?;
    let has_html = clipboard.has_html()?;
    let has_files = clipboard.has_files()?;
    let has_image = clipboard.has_image()?;
    println!("Available types:");
    println!("  has_text:  {has_text}");
    println!("  has_html:  {has_html}");
    println!("  has_files: {has_files}");
    println!("  has_image: {has_image}");
    println!();

    // Try to get text
    if has_text {
        match clipboard.text().await? {
            Some(text) => println!("Text content:\n{text}\n"),
            None => println!("No text content.\n"),
        }
    }

    // Try to get HTML
    if has_html {
        match clipboard.html().await? {
            Some(html) => println!("HTML content:\n{html}\n"),
            None => println!("No HTML content.\n"),
        }
    }

    // Try to get files
    if has_files {
        let files = clipboard.files().await?;
        if files.is_empty() {
            println!("No file content.\n");
        } else {
            println!("File paths:");
            for path in &files {
                println!("  {}", path.display());
            }
            println!();
        }
    }

    // Try to get image
    if has_image {
        match clipboard.image().await? {
            Some(image) => {
                println!(
                    "Image: {}x{} ({} bytes)",
                    image.width(),
                    image.height(),
                    image.bytes().len()
                );

                // Save to file for preview
                match image::save_buffer(
                    "clipboard_preview.png",
                    image.bytes(),
                    image.width(),
                    image.height(),
                    image::ColorType::Rgba8,
                ) {
                    Ok(()) => println!("Saved to clipboard_preview.png"),
                    Err(e) => println!("Failed to save: {e}"),
                }
            }
            None => println!("No image content."),
        }
    }

    Ok(())
}
