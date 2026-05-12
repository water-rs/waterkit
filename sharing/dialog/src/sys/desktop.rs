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
        let level = match dialog.kind {
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
        let level = match dialog.kind {
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

    result
        .map(|file| crate::finalize_selected_file(&dialog, file.path().to_path_buf()))
        .transpose()
}

/// Show a file dialog to open multiple files.
///
/// # Errors
/// Returns an error if the native dialog fails to show or is not supported.
pub async fn show_open_multiple_files(
    dialog: crate::FileDialog,
) -> Result<Option<Vec<std::path::PathBuf>>, DialogError> {
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

    let result = builder.pick_files().await;

    result
        .map(|files| {
            crate::finalize_selected_files(
                &dialog,
                files
                    .into_iter()
                    .map(|file| file.path().to_path_buf())
                    .collect(),
            )
        })
        .transpose()
}

/// A native handle to a selected media file.
#[derive(Debug, Clone)]
pub struct Selection(std::path::PathBuf);

pub async fn load_photo_media(
    handle: Selection,
    requested_media_type: crate::MediaType,
) -> Result<crate::LoadedMedia, DialogError> {
    if matches!(requested_media_type, crate::MediaType::LivePhoto) {
        return Err(DialogError::Unsupported(
            "live photo picker is only supported on iOS".into(),
        ));
    }

    Ok(crate::loaded_media_from_path(handle.0))
}

/// Show a photo picker.
///
/// # Errors
/// Returns an error if the native dialog fails to show or is not supported.
pub async fn show_photo_picker(
    media_type: crate::MediaType,
) -> Result<Option<Selection>, DialogError> {
    if matches!(media_type, crate::MediaType::LivePhoto) {
        return Err(DialogError::Unsupported(
            "live photo picker is only supported on iOS".into(),
        ));
    }

    let mut builder = rfd::AsyncFileDialog::new();

    let exts = match media_type {
        crate::MediaType::Image => vec!["png", "jpg", "jpeg", "gif", "bmp", "webp", "heic"],
        crate::MediaType::Video => vec!["mp4", "mov", "avi", "mkv", "webm"],
        crate::MediaType::LivePhoto => unreachable!("live photo picker must return early"),
    };

    builder = builder.add_filter("Media", &exts);

    let result = builder.pick_file().await;

    Ok(result.map(|f| Selection(f.path().to_path_buf())))
}
