# waterkit-nfc

Cross-platform NFC tag reading and writing for Rust.

Part of the [Waterkit](https://github.com/water-rs/waterkit) ecosystem.

## Features

- Scan for nearby NFC tags
- Read and write NDEF messages
- Create well-known text and URI records
- Tag type identification (Type 1-5, MIFARE Classic)

## Platform Support

| Platform | Status |
|----------|--------|
| iOS      | Native (CoreNFC via Swift bridge) |
| macOS    | Not available (no NFC hardware) |
| Android  | Native (NfcAdapter via JNI/Kotlin) |
| Windows  | Not supported (backend pending) |
| Linux    | Not supported (backend pending) |

## Usage

```rust
use waterkit_nfc::{NfcReader, NdefMessage, NdefRecord, is_available};

async fn example() -> Result<(), waterkit_nfc::NfcError> {
    if !is_available() {
        return Err(waterkit_nfc::NfcError::NotAvailable);
    }

    // Start scanning for tags
    let (reader, rx) = NfcReader::start_session("Hold your device near an NFC tag").await?;

    // Receive discovered tags
    let tag = rx.recv().await.unwrap();

    // Write an NDEF message
    let message = NdefMessage::new()
        .with_record(NdefRecord::text("Hello from Rust"))
        .with_record(NdefRecord::uri("https://example.com"));
    reader.write(message).await?;

    reader.stop();
    Ok(())
}
```

## License

MIT OR Apache-2.0
