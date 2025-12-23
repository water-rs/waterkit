use crate::{Dialog, DialogType};
use native_dialog::{MessageDialog, MessageType};
use futures::channel::oneshot;

/// Show an alert dialog.
pub async fn show_alert(dialog: Dialog) -> Result<(), String> {
    let (tx, rx) = oneshot::channel();
    
    std::thread::spawn(move || {
        let type_ = match dialog.type_ {
            DialogType::Info => MessageType::Info,
            DialogType::Warning => MessageType::Warning,
            DialogType::Error => MessageType::Error,
        };

        let res = MessageDialog::new()
            .set_type(type_)
            .set_title(&dialog.title)
            .set_text(&dialog.message)
            .show_alert()
            .map_err(|e| e.to_string());
        
        let _ = tx.send(res);
    });

    rx.await.map_err(|_| "Cancelled".to_string())?
}

/// Show a confirmation dialog.
pub async fn show_confirm(dialog: Dialog) -> Result<bool, String> {
    let (tx, rx) = oneshot::channel();
    
    std::thread::spawn(move || {
        let type_ = match dialog.type_ {
            DialogType::Info => MessageType::Info,
            DialogType::Warning => MessageType::Warning,
            DialogType::Error => MessageType::Error,
        };

        let res = MessageDialog::new()
            .set_type(type_)
            .set_title(&dialog.title)
            .set_text(&dialog.message)
            .show_confirm()
            .map_err(|e| e.to_string());
        
        let _ = tx.send(res);
    });

    rx.await.map_err(|_| "Cancelled".to_string())?
}

/// Show a file dialog to open a single file.
pub async fn show_open_single_file(dialog: crate::FileDialog) -> Result<Option<std::path::PathBuf>, String> {
    let mut builder = rfd::AsyncFileDialog::new();
    
    if let Some(location) = &dialog.location {
        builder = builder.set_directory(location);
    }
    
    if let Some(title) = &dialog.title {
        builder = builder.set_title(title);
    }
    
    for (name, extensions) in &dialog.filters {
        let exts: Vec<&str> = extensions.iter().map(|s| s.as_str()).collect();
        builder = builder.add_filter(name, &exts);
    }

    let result = builder.pick_file().await;
    
    Ok(result.map(|f| f.path().to_path_buf()))
}
