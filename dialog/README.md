# Waterkit Dialog

Native system dialogs and file pickers.

## Features

- **Alerts**: Information, Warning, Error messages.
- **Confirmations**: Yes/No, OK/Cancel prompts.
- **File Picking**: Open File, Open Directory, Save File.
- **Filters**: Filter files by extension.

## Installation

```toml
[dependencies]
waterkit-dialog = "0.1"
```

## Platform Support

| Platform | Backend |
| :--- | :--- |
| **macOS** | `rfd` (Native Cocoa) |
| **iOS** | `UIAlertController` (Swift) |
| **Android** | `AlertDialog` (Kotlin/JNI) |
| **Windows/Linux** | `rfd` (Native wrappers) |

## Usage

### Simple Alert

```rust
use waterkit_dialog::{Alert, Button};

async fn show_alert() {
    Alert::new("Welcome")
        .message("Hello from Rust!")
        .button(Button::default("OK"))
        .show()
        .await;
}
```

### File Picker (Desktop)

```rust
use waterkit_dialog::FileDialog;

async fn pick_file() {
    let file = FileDialog::new()
        .add_filter("Images", &["png", "jpg"])
        .pick_file()
        .await;
    
    if let Some(path) = file {
        println!("Selected: {:?}", path);
    }
}
```
