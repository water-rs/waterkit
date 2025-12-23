use waterkit_dialog::{Alert, AlertType};

#[tokio::main]
async fn main() {
    println!("Testing Alert...");
    
    /*
    // Test Info Alert
    println!("Showing Info Alert...");
    Alert::new("Alert Test", "This is a native alert on macOS.")
        .with_type(AlertType::Info)
        .show()
        .await
        .expect("Failed to show alert");
    println!("Info Alert dismissed.");

    // Test Confirm Dialog
    println!("Showing Confirm Dialog...");
    let confirmed = Alert::new("Confirmation", "Do you like Rust?")
        .with_type(AlertType::Warning) // Use Warning icon for fun
        .show_confirm()
        .await
        .expect("Failed to show confirm");
    
    println!("Confirmed: {}", confirmed);
    
    if confirmed {
        Alert::new("Great!", "You selected Yes/OK.")
            .show()
            .await
            .expect("Failed to show result");
    } else {
        Alert::new("Oh no...", "You selected No/Cancel.")
            .with_type(AlertType::Error)
            .show()
            .await
            .expect("Failed to show result");
    }
    */

    // Test Printer Dialog
    println!("Showing Printer Dialog...");
    use waterkit_dialog::PrinterDialog;
    let printed = PrinterDialog::new()
        .show()
        .await
        .expect("Failed to show printer dialog");
    println!("Printer Dialog result: {}", printed);

    // Test File Picker
    println!("Showing File Picker...");
    use waterkit_dialog::FileDialog;
    let file = FileDialog::new()
        .with_title("Select a file")
        .add_filter("Text", &["txt", "rs"])
        .show_open_single_file()
        .await
        .expect("Failed to show file picker");
    println!("File selected: {:?}", file);
}
