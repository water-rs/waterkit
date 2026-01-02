//! Clipboard reading demo.

#[tokio::main]
async fn main() -> Result<(), waterkit_clipboard::ClipboardError> {
    println!("Reading clipboard...\n");

    let clipboard = waterkit_clipboard::Clipboard::new()?;

    // Show available types
    println!("Available types:");
    println!("  has_text:  {}", clipboard.has_text());
    println!("  has_html:  {}", clipboard.has_html());
    println!("  has_files: {}", clipboard.has_files());
    println!("  has_image: {}", clipboard.has_image());
    println!();

    // Try to get text
    if clipboard.has_text() {
        match clipboard.text().await? {
            Some(text) => println!("Text content:\n{text}\n"),
            None => println!("No text content.\n"),
        }
    }

    // Try to get HTML
    if clipboard.has_html() {
        match clipboard.html().await? {
            Some(html) => println!("HTML content:\n{html}\n"),
            None => println!("No HTML content.\n"),
        }
    }

    // Try to get files
    if clipboard.has_files() {
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
    if clipboard.has_image() {
        match clipboard.image().await? {
            Some(image) => {
                println!(
                    "Image: {}x{} ({} bytes)",
                    image.width,
                    image.height,
                    image.bytes.len()
                );

                // Save to file for preview
                match image::save_buffer(
                    "clipboard_preview.png",
                    &image.bytes,
                    image.width,
                    image.height,
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
