use waterkit_dialog::FileDialog;

#[tokio::main]
async fn main() {
    println!("Testing Dialog...");

    // Test File Picker
    println!("Showing File Picker...");
    match FileDialog::new()
        .with_title("Select a file")
        .add_filter("Text", &["txt", "rs"])
        .show_open_single_file()
        .await
    {
        Ok(Some(path)) => println!("File selected: {:?}", path),
        Ok(None) => println!("No file selected (cancelled)."),
        Err(e) => println!("Error showing file picker: {}", e),
    }

    // Test Photo Picker
    println!("\nShowing Photo Picker (Images)...");
    let picker = waterkit_dialog::PhotoPicker::new()
        .with_media_type(waterkit_dialog::MediaType::Image);

    match picker.pick().await {
        Ok(Some(handle)) => {
            println!("Photo selected (handle received). Loading...");
            match handle.load().await {
                Ok(path) => println!("Photo loaded at: {:?}", path),
                Err(e) => println!("Error loading photo: {}", e),
            }
        }
        Ok(None) => println!("No photo selected (cancelled)."),
        Err(e) => println!("Error showing photo picker: {}", e),
    }
}
