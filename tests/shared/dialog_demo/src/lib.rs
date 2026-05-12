//! Dialog demonstration utility.
use waterkit_dialog::{Dialog, DialogType};

/// Runs the dialog demonstration.
pub async fn run() {
    println!("Running Dialog Demo...");

    // Show Info
    let _ = Dialog::new("Demo", "Hello from shared crate!")
        .with_type(DialogType::Info)
        .show()
        .await;

    // Show Confirm
    let result = Dialog::new("Confirm", "Do you accept?")
        .with_type(DialogType::Warning)
        .show_confirm()
        .await
        .unwrap_or(false);

    if result {
        let _ = Dialog::new("Result", "Accepted!").show().await;
    } else {
        let _ = Dialog::new("Result", "Declined!")
            .with_type(DialogType::Error)
            .show()
            .await;
    }

    // Photo Picker Demo
    let _ = Dialog::new("Photo Picker", "Next: Pick a photo")
        .show()
        .await;

    let picker =
        waterkit_dialog::PhotoPicker::new().with_media_type(waterkit_dialog::MediaType::Image);

    match picker.pick().await {
        Ok(Some(handle)) => {
            let _ = Dialog::new("Picker Result", "Media selected. Loading...")
                .show()
                .await;
            match handle.load().await {
                Ok(path) => {
                    let msg = format!("Loaded: {}", path.display());
                    let _ = Dialog::new("Success", &msg).show().await;
                }
                Err(e) => {
                    let msg = format!("Load Error: {e}");
                    let _ = Dialog::new("Error", &msg)
                        .with_type(DialogType::Error)
                        .show()
                        .await;
                }
            }
        }
        Ok(None) => {
            let _ = Dialog::new("Picker Result", "No selection").show().await;
        }
        Err(e) => {
            let msg = format!("Error: {e}");
            let _ = Dialog::new("Picker Error", &msg)
                .with_type(DialogType::Error)
                .show()
                .await;
        }
    }
}
