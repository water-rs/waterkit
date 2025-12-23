use waterkit_dialog::{Dialog, DialogType};

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
        let _ = Dialog::new("Result", "Accepted!")
            .show()
            .await;
    } else {
        let _ = Dialog::new("Result", "Declined!")
            .with_type(DialogType::Error)
            .show()
            .await;
    }
}
