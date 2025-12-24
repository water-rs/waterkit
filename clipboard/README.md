# Waterkit Clipboard

System clipboard access for Rust applications.

## Features

- **Text**: Read and write plain text.
- **Images**: (Experimental) Read and write images.
- **Reactive**: (Roadmap) Listen for clipboard changes.

## Installation

```toml
[dependencies]
waterkit-clipboard = "0.1"
# OR
waterkit = { version = "0.1", features = ["clipboard"] }
```

## Platform Support

| Platform | Backend |
| :--- | :--- |
| **macOS** | `NSPasteboard` / `arboard` |
| **iOS** | `UIPasteboard` (Swift Bridge) |
| **Android** | `ClipboardManager` (Kotlin/JNI) |
| **Windows/Linux** | `arboard` |

## Usage

```rust
use waterkit_clipboard::Clipboard;

async fn copy_paste() {
    let clipboard = Clipboard::new().await.unwrap();
    
    // Write
    clipboard.set_text("Hello World").await.unwrap();
    
    // Read
    let content = clipboard.get_text().await.unwrap();
    println!("Clipboard content: {}", content);
}
```
