# Waterkit Clipboard

System clipboard access for Rust applications.

## Features

- **Text**: Read and write plain text.
- **Images**: (Experimental) Read and write images.
- **Files**: Read and write file paths.
- **Custom data**: Read and write arbitrary MIME-typed payloads.
- **Reactive**: Listen for clipboard changes.
- **Primary selection (Linux)**: Read and write the PRIMARY selection text on
  X11 and Wayland — the text last selected, pasted with the middle mouse
  button.

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
| **macOS** | `clipboard-rs` (`NSPasteboard`) |
| **iOS** | `UIPasteboard` (Swift Bridge) |
| **Android** | `ClipboardManager` (Kotlin/JNI) |
| **Windows** | `clipboard-rs` (Win32) |
| **Linux** | `clipboard-rs` for CLIPBOARD; `arboard` (X11 + Wayland data-control) for PRIMARY |

## Usage

```rust
use waterkit_clipboard::Clipboard;

async fn copy_paste() -> Result<(), waterkit_clipboard::ClipboardError> {
    let mut clipboard = Clipboard::new()?;

    // Write
    clipboard.set_text("Hello World")?;

    // Read
    if let Some(text) = clipboard.text().await? {
        println!("Clipboard content: {text}");
    }
    Ok(())
}
```

## Primary Selection (Linux only)

Linux desktops keep a second selection, PRIMARY, holding the text last
selected; terminals and editors write it on select and read it on
middle-click. `PrimarySelection` reads and writes its text under both X11
and Wayland:

```rust
use waterkit_clipboard::PrimarySelection;

async fn primary() -> Result<(), waterkit_clipboard::ClipboardError> {
    let mut primary = PrimarySelection::new()?;
    primary.set_text("selected text")?;
    if let Some(text) = primary.text().await? {
        println!("PRIMARY: {text}");
    }
    Ok(())
}
```

After `set_text` the crate owns the selection and keeps serving paste
requests: on X11 an in-process worker thread answers `SelectionRequest`s
until another client claims PRIMARY or the handle is dropped; on Wayland a
forked child serves data-control requests until another client claims it.
Wayland needs a compositor with a data-control protocol offering a primary
selection (`zwlr_data_control_manager_v1` version 2+, or
`ext_data_control_manager_v1`); compositors without one expose no PRIMARY and
operations error.

The API is `cfg`-gated to `target_os = "linux"`: other platforms do not get
it at all — there is no stub and no CLIPBOARD emulation.

## License

Licensed under either of [Apache-2.0](LICENSE-APACHE) or
[MIT](LICENSE-MIT) at your option.
