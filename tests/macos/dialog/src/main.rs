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
}
