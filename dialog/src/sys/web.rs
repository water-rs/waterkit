use std::path::{Path, PathBuf};

use crate::{Dialog, DialogError, DialogType};
use rfd::{AsyncFileDialog, AsyncMessageDialog, MessageButtons, MessageDialogResult, MessageLevel};
use waterkit_fs::WaterFs;

const FILE_PICKER_CACHE_SUBDIR: &str = "waterkit/dialog/open";
const PHOTO_PICKER_CACHE_SUBDIR: &str = "waterkit/dialog/photo-picker";

#[derive(Debug, Clone)]
pub struct Selection(rfd::FileHandle);

pub async fn show_alert(dialog: Dialog) -> Result<(), DialogError> {
    AsyncMessageDialog::new()
        .set_level(message_level(dialog.type_))
        .set_title(&dialog.title)
        .set_description(&dialog.message)
        .set_buttons(MessageButtons::Ok)
        .show()
        .await;
    Ok(())
}

pub async fn show_confirm(dialog: Dialog) -> Result<bool, DialogError> {
    let result = AsyncMessageDialog::new()
        .set_level(message_level(dialog.type_))
        .set_title(&dialog.title)
        .set_description(&dialog.message)
        .set_buttons(MessageButtons::OkCancel)
        .show()
        .await;
    Ok(matches!(
        result,
        MessageDialogResult::Ok | MessageDialogResult::Yes
    ))
}

pub async fn show_open_single_file(
    dialog: crate::FileDialog,
) -> Result<Option<PathBuf>, DialogError> {
    let result = build_file_dialog(&dialog).pick_file().await;

    match result {
        Some(file) => import_browser_file(&dialog, &file, Path::new(FILE_PICKER_CACHE_SUBDIR))
            .await
            .map(Some),
        None => Ok(None),
    }
}

pub async fn show_open_multiple_files(
    dialog: crate::FileDialog,
) -> Result<Option<Vec<PathBuf>>, DialogError> {
    let result = build_file_dialog(&dialog).pick_files().await;

    match result {
        Some(files) => {
            let mut paths = Vec::with_capacity(files.len());
            for file in files {
                paths.push(
                    import_browser_file(&dialog, &file, Path::new(FILE_PICKER_CACHE_SUBDIR))
                        .await?,
                );
            }
            Ok(Some(paths))
        }
        None => Ok(None),
    }
}

pub async fn show_photo_picker(
    media_type: crate::MediaType,
) -> Result<Option<Selection>, DialogError> {
    if matches!(media_type, crate::MediaType::LivePhoto) {
        return Err(DialogError::Unsupported(
            "live photo picker is only supported on iOS".into(),
        ));
    }

    let result = AsyncFileDialog::new()
        .add_filter("Media", &media_extensions(media_type))
        .pick_file()
        .await;

    Ok(result.map(Selection))
}

pub async fn load_photo_media(
    handle: Selection,
    requested_media_type: crate::MediaType,
) -> Result<crate::LoadedMedia, DialogError> {
    if matches!(requested_media_type, crate::MediaType::LivePhoto) {
        return Err(DialogError::Unsupported(
            "live photo picker is only supported on iOS".into(),
        ));
    }

    let path = import_browser_file(
        &crate::FileDialog::new().import_to_cache_subdir(PHOTO_PICKER_CACHE_SUBDIR),
        &handle.0,
        Path::new(PHOTO_PICKER_CACHE_SUBDIR),
    )
    .await?;

    Ok(crate::loaded_media_from_path(path))
}

fn build_file_dialog(dialog: &crate::FileDialog) -> AsyncFileDialog {
    let mut builder = AsyncFileDialog::new();

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

    builder
}

async fn import_browser_file(
    dialog: &crate::FileDialog,
    file: &rfd::FileHandle,
    default_subdir: &Path,
) -> Result<PathBuf, DialogError> {
    let bytes = file.read().await;
    let cache_subdir = dialog
        .import_to_cache_subdir
        .as_deref()
        .unwrap_or(default_subdir);
    WaterFs::import_bytes_to_cache(&bytes, &file.file_name(), cache_subdir)
        .await
        .map_err(DialogError::from)
}

fn media_extensions(media_type: crate::MediaType) -> Vec<&'static str> {
    match media_type {
        crate::MediaType::Image => vec!["png", "jpg", "jpeg", "gif", "bmp", "webp", "heic"],
        crate::MediaType::Video => vec!["mp4", "mov", "avi", "mkv", "webm"],
        crate::MediaType::LivePhoto => unreachable!("live photo picker must return early"),
    }
}

fn message_level(type_: DialogType) -> MessageLevel {
    match type_ {
        DialogType::Info => MessageLevel::Info,
        DialogType::Warning => MessageLevel::Warning,
        DialogType::Error => MessageLevel::Error,
    }
}
