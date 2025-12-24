//! Apple platform (iOS/macOS) clipboard implementation using swift-bridge.

use crate::ImageData;
use std::borrow::Cow;

#[swift_bridge::bridge]
mod ffi {
    #[swift_bridge(swift_repr = "struct")]
    struct SwiftImageData {
        width: usize,
        height: usize,
        bytes: Vec<u8>,
        is_valid: bool,
    }

    extern "Swift" {
        fn clipboard_get_text() -> Option<String>;
        fn clipboard_set_text(text: String);
        fn clipboard_get_image() -> SwiftImageData;
        fn clipboard_set_image(image: SwiftImageData);
    }
}

/// Get text from the Apple system clipboard.
#[must_use]
pub fn get_text() -> Option<String> {
    ffi::clipboard_get_text()
}

/// Set text to the Apple system clipboard.
pub fn set_text(text: String) {
    ffi::clipboard_set_text(text);
}

/// Get image from the Apple system clipboard.
#[must_use]
pub fn get_image() -> Option<ImageData> {
    let image = ffi::clipboard_get_image();
    if !image.is_valid {
        return None;
    }
    Some(ImageData {
        width: image.width,
        height: image.height,
        bytes: Cow::Owned(image.bytes),
    })
}

/// Set image to the Apple system clipboard.
pub fn set_image(image: ImageData) {
    let swift_image = ffi::SwiftImageData {
        width: image.width,
        height: image.height,
        bytes: image.bytes.into_owned(),
        is_valid: true,
    };
    ffi::clipboard_set_image(swift_image);
}
