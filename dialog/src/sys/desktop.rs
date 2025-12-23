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
    let (tx, rx) = oneshot::channel();
    
    std::thread::spawn(move || {
        // Initial builder
        let mut builder = native_dialog::FileDialog::new();
        
        if let Some(location) = &dialog.location {
            builder = builder.set_location(location);
        }
        
        // native-dialog add_filter signature: fn add_filter(self, name: &str, extensions: &[&str]) -> Self
        // It should own the data. If it takes references that must outlive the builder, that's annoying.
        // Let's assume the issue was creating the vector inside the loop.
        // We'll collect all extensions first to keep them alive if strictly needed,
        // but native_dialog usually copies?
        // Actually, looking at common rust bindings, they often take &[&str].
        // If native-dialog keeps the reference, we are in trouble with dynamic filters.
        // However, looking at crate usage, usually people use static strings.
        // Let's try to see if we can trick it or if I was just wrong about the error context.
        // Error: `exts` dropped here while still borrowed. `builder` borrow used later.
        // This implies builder stores the reference.
        // native-dialog 0.7 might have `add_filter` that takes `&lt;'_&gt;`.
        // If so, we can't use dynamic strings easily without leaking/arena? 
        // Or maybe we construct a vector of vectors of strings outside?
        
        // Let's try to prepare the data structures outside.
        let filters_data: Vec<(String, Vec<String>)> = dialog.filters.clone();
        let filters_slices: Vec<(String, Vec<&str>)> = filters_data.iter()
            .map(|(n, exts)| (n.clone(), exts.iter().map(|s| s.as_str()).collect()))
            .collect();

        for (name, exts) in &filters_slices {
             builder = builder.add_filter(name, exts);
        }

        // Wait, if builder keeps ref to exts, then exts must live as long as builder.
        // If I move builder into a block where filters_slices lives, maybe it works?
        // But the builder is returned/used? result is used.
        // No, `builder.show_open_single_file()` consumes builder.
        // So filters_slices must live until `show_open_single_file` is called.
        // The previous loop dropped `exts` inside the loop (per iteration).
        
        let res = builder.show_open_single_file()
            .map_err(|e| e.to_string());
            
        let _ = tx.send(res);
    });

    rx.await.map_err(|_| "Cancelled".to_string())?
}
