use crate::{Dialog, DialogType, DialogError};
use futures::channel::oneshot;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};



#[derive(Debug, Clone)]
pub struct Selection(u64);

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn callbacks() -> &'static Mutex<HashMap<u64, oneshot::Sender<bool>>> {
    static LOCK: OnceLock<Mutex<HashMap<u64, oneshot::Sender<bool>>>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(HashMap::new()))
}

fn picker_callbacks() -> &'static Mutex<HashMap<u64, oneshot::Sender<Option<Selection>>>> {
    static LOCK: OnceLock<Mutex<HashMap<u64, oneshot::Sender<Option<Selection>>>>> =
        OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(HashMap::new()))
}

fn load_callbacks() -> &'static Mutex<HashMap<u64, oneshot::Sender<Option<String>>>> {
    static LOCK: OnceLock<Mutex<HashMap<u64, oneshot::Sender<Option<String>>>>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(HashMap::new()))
}

#[swift_bridge::bridge]
mod ffi {
    extern "Swift" {
        fn show_alert_bridge(title: &str, message: &str, type_str: &str, cb_id: u64);
        fn show_confirm_bridge(title: &str, message: &str, type_str: &str, cb_id: u64);
        fn show_photo_picker_bridge(media_type: &str, cb_id: u64);
        fn load_media_bridge(handle_id: u64, cb_id: u64);
    }

    extern "Rust" {
        fn on_dialog_result(cb_id: u64, result: bool);
        fn on_photo_picker_result(cb_id: u64, handle_id: Option<u64>);
        fn on_load_media_result(cb_id: u64, path: Option<String>);
    }
}

fn on_dialog_result(cb_id: u64, result: bool) {
    if let Ok(mut map) = callbacks().lock() {
        if let Some(tx) = map.remove(&cb_id) {
            let _ = tx.send(result);
        }
    }
}

fn on_photo_picker_result(cb_id: u64, handle_id: Option<u64>) {
    if let Ok(mut map) = picker_callbacks().lock() {
        if let Some(tx) = map.remove(&cb_id) {
            let _ = tx.send(handle_id.map(Selection));
        }
    }
}

fn on_load_media_result(cb_id: u64, path: Option<String>) {
    if let Ok(mut map) = load_callbacks().lock() {
        if let Some(tx) = map.remove(&cb_id) {
            let _ = tx.send(path);
        }
    }
}


pub async fn show_alert(dialog: Dialog) -> Result<(), DialogError> {
    let (tx, rx) = oneshot::channel();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

    callbacks().lock().unwrap().insert(id, tx);

    let type_str = match dialog.type_ {
        DialogType::Info => "info",
        DialogType::Warning => "warning",
        DialogType::Error => "error",
    };

    ffi::show_alert_bridge(&dialog.title, &dialog.message, type_str, id);

    let _ = rx.await;
    Ok(())
}

pub async fn show_confirm(dialog: Dialog) -> Result<bool, DialogError> {
    let (tx, rx) = oneshot::channel();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

    callbacks().lock().unwrap().insert(id, tx);

    let type_str = match dialog.type_ {
        DialogType::Info => "info",
        DialogType::Warning => "warning",
        DialogType::Error => "error",
    };

    ffi::show_confirm_bridge(&dialog.title, &dialog.message, type_str, id);

    rx.await.map_err(|_| DialogError::Cancelled)
}

pub async fn show_photo_picker(
    picker: crate::PhotoPicker,
) -> Result<Option<Selection>, DialogError> {
    let (tx, rx) = oneshot::channel();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

    picker_callbacks().lock().unwrap().insert(id, tx);

    let media_type = match picker.media_type {
        crate::MediaType::Image => "image",
        crate::MediaType::Video => "video",
        crate::MediaType::LivePhoto => "livephoto",
    };

    ffi::show_photo_picker_bridge(media_type, id);

    rx.await.map_err(|_| DialogError::Cancelled)
}

pub async fn load_media(handle: Selection) -> Result<std::path::PathBuf, DialogError> {
    let (tx, rx) = oneshot::channel();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

    load_callbacks().lock().unwrap().insert(id, tx);

    ffi::load_media_bridge(handle.0, id);

    let res = rx.await.map_err(|_| DialogError::Cancelled)?;
    match res {
        Some(path) => Ok(std::path::PathBuf::from(path)),
        None => Err(DialogError::PlatformError("Failed to load media (conversion failed)".to_string())),
    }
}
