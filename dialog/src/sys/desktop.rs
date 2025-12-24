use crate::{Dialog, DialogType};
use futures::channel::oneshot;
use rfd::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};

/// Show an alert dialog.
///
/// # Errors
/// Returns an error if the native dialog fails to show or is not supported.
pub async fn show_alert(dialog: Dialog) -> Result<(), String> {
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

        let _ = tx.send(Ok::<(), String>(()));
    });

    rx.await.map_err(|_| "Cancelled".to_string())?
}

/// Show a confirmation dialog.
///
/// # Errors
/// Returns an error if the native dialog fails to show or is not supported.
pub async fn show_confirm(dialog: Dialog) -> Result<bool, String> {
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

        let _ = tx.send(Ok::<bool, String>(confirmed));
    });

    rx.await.map_err(|_| "Cancelled".to_string())?
}

/// Show a file dialog to open a single file.
///
/// # Errors
/// Returns an error if the native dialog fails to show or is not supported.
pub async fn show_open_single_file(
    dialog: crate::FileDialog,
) -> Result<Option<std::path::PathBuf>, String> {
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
