use crate::{Alert, AlertType};
use native_dialog::{MessageDialog, MessageType};
use futures::channel::oneshot;

/// Show an alert dialog.
pub async fn show_alert(alert: Alert) -> Result<(), String> {
    let (tx, rx) = oneshot::channel();
    
    std::thread::spawn(move || {
        let type_ = match alert.type_ {
            AlertType::Info => MessageType::Info,
            AlertType::Warning => MessageType::Warning,
            AlertType::Error => MessageType::Error,
        };

        let res = MessageDialog::new()
            .set_type(type_)
            .set_title(&alert.title)
            .set_text(&alert.message)
            .show_alert()
            .map_err(|e| e.to_string());
        
        let _ = tx.send(res);
    });

    rx.await.map_err(|_| "Cancelled".to_string())?
}

/// Show a confirmation dialog.
pub async fn show_confirm(alert: Alert) -> Result<bool, String> {
    let (tx, rx) = oneshot::channel();
    
    std::thread::spawn(move || {
        let type_ = match alert.type_ {
            AlertType::Info => MessageType::Info,
            AlertType::Warning => MessageType::Warning,
            AlertType::Error => MessageType::Error,
        };

        let res = MessageDialog::new()
            .set_type(type_)
            .set_title(&alert.title)
            .set_text(&alert.message)
            .show_confirm()
            .map_err(|e| e.to_string());
        
        let _ = tx.send(res);
    });

    rx.await.map_err(|_| "Cancelled".to_string())?
}
