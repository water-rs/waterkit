use crate::{Dialog, DialogError, DialogType};
use futures::channel::oneshot;
use rfd::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};

/// Show an alert dialog.
///
/// # Errors
/// Returns an error if the native dialog fails to show or is not supported.
pub async fn show_alert(dialog: Dialog) -> Result<(), DialogError> {
    let (tx, rx) = oneshot::channel();

    std::thread::spawn(move || {
        let level = match dialog.type_ {
            DialogType::Info => MessageLevel::Info,
            DialogType::Warning => MessageLevel::Warning,
            DialogType::Error => MessageLevel::Error,
        };

        MessageDialog::new()
            .set_level(level)
            .set_title(&dialog.title)
            .set_description(&dialog.message)
            .set_buttons(MessageButtons::Ok)
            .show();

        let _ = tx.send(());
    });

    rx.await
        .map_err(|_| DialogError::PlatformError("Dialog panicked or channel closed".into()))
}

/// Show a confirmation dialog.
///
/// # Errors
/// Returns an error if the native dialog fails to show or is not supported.
pub async fn show_confirm(dialog: Dialog) -> Result<bool, DialogError> {
    let (tx, rx) = oneshot::channel();

    std::thread::spawn(move || {
        let level = match dialog.type_ {
            DialogType::Info => MessageLevel::Info,
            DialogType::Warning => MessageLevel::Warning,
            DialogType::Error => MessageLevel::Error,
        };

        let result = MessageDialog::new()
            .set_level(level)
            .set_title(&dialog.title)
            .set_description(&dialog.message)
            .set_buttons(MessageButtons::OkCancel)
            .show();

        let confirmed = matches!(result, MessageDialogResult::Ok | MessageDialogResult::Yes);

        let _ = tx.send(confirmed);
    });

    rx.await
        .map_err(|_| DialogError::PlatformError("Dialog panicked or channel closed".into()))
}

/// Show a file dialog to open a single file.
///
/// # Errors
/// Returns an error if the native dialog fails to show or is not supported.
pub async fn show_open_single_file(
    dialog: crate::FileDialog,
) -> Result<Option<std::path::PathBuf>, DialogError> {
    let mut builder = rfd::AsyncFileDialog::new();

    if let Some(location) = &dialog.location {
        builder = builder.set_directory(location);
    }

    if let Some(title) = &dialog.title {
        builder = builder.set_title(title);
    }

    for (name, extensions) in &dialog.filters {
        let exts: Vec<&str> = extensions.iter().map(std::string::String::as_str).collect();
        builder = builder.add_filter(name, &exts);
    }

    let result = builder.pick_file().await;

    Ok(result.map(|f| f.path().to_path_buf()))
}

/// A native handle to a selected media file.
#[derive(Debug, Clone)]
pub struct Selection(std::path::PathBuf);

/// Load the media from a handle.
pub async fn load_media(handle: Selection) -> Result<std::path::PathBuf, DialogError> {
    Ok(handle.0)
}

/// Show a photo picker.
///
/// # Errors
/// Returns an error if the native dialog fails to show or is not supported.
pub async fn show_photo_picker(
    picker: crate::PhotoPicker,
) -> Result<Option<Selection>, DialogError> {
    let mut builder = rfd::AsyncFileDialog::new();

    let exts = match picker.media_type {
        crate::MediaType::Image => vec!["png", "jpg", "jpeg", "gif", "bmp", "webp", "heic"],
        crate::MediaType::Video => vec!["mp4", "mov", "avi", "mkv", "webm"],
        crate::MediaType::LivePhoto => vec!["png", "jpg", "jpeg", "heic", "mov"], // Fallback
    };

    builder = builder.add_filter("Media", &exts);

    let result = builder.pick_file().await;

    Ok(result.map(|f| Selection(f.path().to_path_buf())))
}
