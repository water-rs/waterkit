# Waterkit

**Waterkit** is a comprehensive, modular collection of cross-platform utilities designed to empower Rust applications with native system capabilities. It bridges the gap between Rust and platform-specific APIs (iOS, Android, macOS, Windows, Linux), allowing you to build rich, native-feeling applications with a unified Rust interface.

## Modules

Waterkit is organized into focused, independent crates. You can use the main `waterkit` crate with feature flags, or depend on individual crates directly.

| Feature / Crate | Description |
| :--- | :--- |
| **[Audio](audio)** | Cross-platform audio playback and recording. |
| **[Background](background)** | Background refresh and heavy background task scheduling APIs. |
| **[Biometric](biometric)** | TouchID, FaceID, Windows Hello, and native biometric authentication. |
| **[Bluetooth](bluetooth)** | BLE scanning, device discovery, and connection management. |
| **[Calendar](calendar)** | Native calendar event read/write integrations. |
| **[Camera](camera)** | Camera streaming and capture (Webcam, AVFoundation, Camera2). |
| **[Clipboard](clipboard)** | System clipboard access for text and images. |
| **[Codec](codec)** | Low-level hardware video/audio encoding and decoding. |
| **[Contacts](contacts)** | Native contacts query and synchronization helpers. |
| **[Deeplink](deeplink)** | URL scheme and universal-link/deep-link handling. |
| **[Dialog](dialog)** | Native system alert dialogs, file pickers, and prompts. |
| **[FS](fs)** | File system helpers, sandboxing, and file picking. |
| **[Haptic](haptic)** | Haptic feedback and vibration control. |
| **[Health](health)** | Health data integration (HealthKit / Health Connect). |
| **[Location](location)** | GPS and location services (CoreLocation, LocationManager, etc.). |
| **[NFC](nfc)** | NFC read/write and tag interaction workflows. |
| **[Notification](notification)** | Local system notifications. |
| **[Permission](permission)** | Unified API for requesting system permissions (Camera, Mic, Location, etc.). |
| **[Regional](regional)** | Locale, preferred languages, region, and timezone context helpers. |
| **[Passkey](passkey)** | Native passkey registration/authentication ceremonies with ergonomic WebAuthn helpers. |
| **[Screen](screen)** | Screen capture and display information. |
| **[Secret](secret)** | Secure storage (Keychain, Keystore, Credential Locker). |
| **[Sensor](sensor)** | Access to device sensors (Accelerometer, Gyroscope, Magnetometer, etc.). |
| **[Share](share)** | Native share sheet and cross-app content sharing. |
| **[Speech](speech)** | Speech recognition and text-to-speech integrations. |
| **[System](system)** | System information, connectivity status, and thermal info. |
| **[Video](video)** | High-level video playback and processing. |

## Advanced Modern Capabilities

- Hardware-accelerated media pipelines (`codec`, `video`) with platform-native backends and GPU-friendly paths.
- Privacy-first device access flows (`permission`, `biometric`, `secret`) for secure user consent and authentication.
- Deep OS integrations (`bluetooth`, `nfc`, `health`, `contacts`, `calendar`, `notification`, `deeplink`, `share`, `speech`).
- Async-first APIs across modules to fit modern concurrent Rust application architecture.
- `full` feature as a complete capability bundle, continuously validated by automated feature-surface tests.

## 📦 Installation

Add `waterkit` to your `Cargo.toml`. We recommend enabling only the features you need to keep compile times low.

```toml
[dependencies]
waterkit = { version = "0.1", features = ["location", "dialog", "haptic"] }
```

### Full Installation
If you want everything:
```toml
[dependencies]
waterkit = { version = "0.1", features = ["full"] }
```

## Platform Support

Waterkit uses a mix of pure Rust crates and native bridges (Swift/Kotlin) to achieve maximum compatibility and performance.

| Platform | Support | Implementation Details |
| :--- | :--- | :--- |
| **macOS** | ✅ First-class | Native Swift/ObjC, Frameworks |
| **iOS** | ✅ First-class | Swift Bridge, Native Frameworks |
| **Android** | ✅ First-class | JNI, Kotlin Bridge |
| **Windows** | ✅ Supported | `windows-rs`, Win32 APIs |
| **Linux** | 🚧 Beta | DBus, various system crates |

## Usage Example

Here's a quick example of using multiple modules together:

```rust
use waterkit::permission::{Permission, PermissionStatus};
use waterkit::location::LocationManager;
use waterkit::dialog::{Alert, Button};

async fn example() {
    // 1. Check Permissions
    let perm = waterkit::permission::check(Permission::Location).await;
    
    if perm != PermissionStatus::Granted {
        // 2. Request if needed
        let status = waterkit::permission::request(Permission::Location).await;
        if status != PermissionStatus::Granted {
            // 3. Show Native Alert
            Alert::new("Permission Denied")
                .message("We need location access to show you the map.")
                .button(Button::default("OK"))
                .show()
                .await;
            return;
        }
    }

    // 4. Use Location
    let location_manager = LocationManager::new().await.unwrap();
    let loc = location_manager.get_current_location().await.unwrap();
    log::info!("Location: {}, {}", loc.latitude, loc.longitude);
}
```

## Contributing

Contributions are welcome! Please check individual crate directories for specific implementation details.

## License

MIT OR Apache-2.0 License
