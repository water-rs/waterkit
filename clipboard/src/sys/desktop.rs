use crate::ImageData;
use arboard::Clipboard;
use std::borrow::Cow;

/// Get text from the clipboard.
pub fn get_text() -> Option<String> {
    Clipboard::new().ok()?.get_text().ok()
}

/// Set text to the clipboard.
pub fn set_text(text: String) {
    if let Ok(mut clipboard) = Clipboard::new() {
        let _ = clipboard.set_text(text);
    }
}

/// Get image from the clipboard.
pub fn get_image() -> Option<ImageData> {
    let mut clipboard = Clipboard::new().ok()?;
    let image = clipboard.get_image().ok()?;
    Some(ImageData {
        width: image.width,
        height: image.height,
        bytes: Cow::Owned(image.bytes.into_owned()),
    })
}

/// Set image to the clipboard.
pub fn set_image(image: ImageData) {
    if let Ok(mut clipboard) = Clipboard::new() {
        let _ = clipboard.set_image(arboard::ImageData {
            width: image.width,
            height: image.height,
            bytes: image.bytes,
        });
    }
}
