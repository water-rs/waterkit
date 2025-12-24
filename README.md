# Waterkit

**Waterkit** is a comprehensive, modular collection of cross-platform utilities designed to empower Rust applications with native system capabilities. It bridges the gap between Rust and platform-specific APIs (iOS, Android, macOS, Windows, Linux), allowing you to build rich, native-feeling applications with a unified Rust interface.

Waterkit is designed to work seamlessly with **WaterUI**, but can be used independently in any Rust project.

## ✨ Modules

Waterkit is organized into focused, independent crates. You can use the main `waterkit` crate with feature flags, or depend on individual crates directly.

| Feature / Crate | Description |
| :--- | :--- |
| **[Audio](audio)** | Cross-platform audio playback and recording. |
| **[Biometric](biometric)** | TouchID, FaceID, Windows Hello, and native biometric authentication. |
| **[Camera](camera)** | Camera streaming and capture (Webcam, AVFoundation, Camera2). |
| **[Clipboard](clipboard)** | System clipboard access for text and images. |
| **[Codec](codec)** | Low-level hardware video/audio encoding and decoding. |
| **[Dialog](dialog)** | Native system alert dialogs, file pickers, and prompts. |
| **[FS](fs)** | File system helpers, sandboxing, and file picking. |
| **[Haptic](haptic)** | Haptic feedback and vibration control. |
| **[Location](location)** | GPS and location services (CoreLocation, LocationManager, etc.). |
| **[Notification](notification)** | Local system notifications. |
| **[Permission](permission)** | Unified API for requesting system permissions (Camera, Mic, Location, etc.). |
| **[Screen](screen)** | Screen capture and display information. |
| **[Secret](secret)** | Secure storage (Keychain, Keystore, Credential Locker). |
| **[Sensor](sensor)** | Access to device sensors (Accelerometer, Gyroscope, Magnetometer, etc.). |
| **[System](system)** | System information, connectivity status, and thermal info. |
| **[Video](video)** | High-level video playback and processing. |

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

## 🍎 Platform Support

Waterkit uses a mix of pure Rust crates and native bridges (Swift/Kotlin) to achieve maximum compatibility and performance.

| Platform | Support | Implementation Details |
| :--- | :--- | :--- |
| **macOS** | ✅ First-class | Native Swift/ObjC, Frameworks |
| **iOS** | ✅ First-class | Swift Bridge, Native Frameworks |
| **Android** | ✅ First-class | JNI, Kotlin Bridge |
| **Windows** | ✅ Supported | `windows-rs`, Win32 APIs |
| **Linux** | 🚧 Beta | DBus, various system crates |

## 🛠️ Usage Example

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
    println!("Location: {}, {}", loc.latitude, loc.longitude);
}
```

## 🤝 Contributing

Contributions are welcome! Please check individual crate directories for specific implementation details.

## 📄 License

MIT License
