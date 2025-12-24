use crate::{Dialog, DialogType};
use futures::channel::oneshot;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn callbacks() -> &'static Mutex<HashMap<u64, oneshot::Sender<bool>>> {
    static LOCK: OnceLock<Mutex<HashMap<u64, oneshot::Sender<bool>>>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(HashMap::new()))
}

#[swift_bridge::bridge]
mod ffi {
    extern "Swift" {
        fn show_alert_bridge(title: &str, message: &str, type_str: &str, cb_id: u64);
        fn show_confirm_bridge(title: &str, message: &str, type_str: &str, cb_id: u64);
    }

    extern "Rust" {
        fn on_dialog_result(cb_id: u64, result: bool);
    }
}

fn on_dialog_result(cb_id: u64, result: bool) {
    if let Ok(mut map) = callbacks().lock() {
        if let Some(tx) = map.remove(&cb_id) {
            let _ = tx.send(result);
        }
    }
}

pub async fn show_alert(dialog: Dialog) -> Result<(), String> {
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

pub async fn show_confirm(dialog: Dialog) -> Result<bool, String> {
    let (tx, rx) = oneshot::channel();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

    callbacks().lock().unwrap().insert(id, tx);

    let type_str = match dialog.type_ {
        DialogType::Info => "info",
        DialogType::Warning => "warning",
        DialogType::Error => "error",
    };

    ffi::show_confirm_bridge(&dialog.title, &dialog.message, type_str, id);

    rx.await.map_err(|_| "Cancelled".to_string())
}
