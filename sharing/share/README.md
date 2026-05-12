# waterkit-share

Cross-platform share sheet and social sharing for Rust.

Part of the [Waterkit](https://github.com/water-rs/waterkit) ecosystem.

## Features

- Share text, URLs, images, and files via the native share sheet
- Builder API for composing multi-item share requests
- Optional subject line for email sharing

## Platform Support

| Platform | Status |
|----------|--------|
| iOS      | Native (UIActivityViewController via Swift bridge) |
| macOS    | Native (NSSharingServicePicker via Swift bridge) |
| Android  | Native (Intent.ACTION_SEND via JNI/Kotlin) |
| Windows  | Native (Windows.ApplicationModel.DataTransfer) |
| Linux    | D-Bus / xdg-open |

## Usage

```rust
use waterkit_share::{ShareSheet, ShareResult};

async fn example() -> Result<(), waterkit_share::ShareError> {
    // Share text
    let result = ShareSheet::text("Check out this app!")
        .subject("Recommendation")
        .show()
        .await?;

    match result {
        ShareResult::Shared => { /* success */ }
        ShareResult::Cancelled => { /* user cancelled */ }
    }

    // Share a URL
    ShareSheet::url("https://example.com").show().await?;

    // Share a file
    ShareSheet::file("/path/to/document.pdf").show().await?;

    Ok(())
}
```

## License

MIT OR Apache-2.0
