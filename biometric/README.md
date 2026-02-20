# Waterkit Biometric

Native biometric authentication (TouchID, FaceID, Fingerprint, Face Unlock) for Rust applications.

## Features

- **Unified API**: Single `authenticate` function for all platforms.
- **Native UI**: Uses the system's standard authentication prompts.
- **Availability checks**: Query whether biometrics are usable before prompting.

## Installation

```toml
[dependencies]
waterkit-biometric = "0.1"
# OR
waterkit = { version = "0.1", features = ["biometric"] }
```

## Platform Support

| Platform | Technology |
| :--- | :--- |
| **macOS** | LocalAuthentication (TouchID) |
| **iOS** | LocalAuthentication (FaceID / TouchID) |
| **Android** | `android.hardware.biometrics.BiometricPrompt` |
| **Windows** | Windows Hello |
| **Linux** | `fprintd` via D-Bus |

## Usage

```rust
use waterkit_biometric::authenticate;

async fn login() {
    // Optional: Check what type is available
    let bio_type = waterkit_biometric::get_biometric_type().await;
    let _ = bio_type;

    // Authenticate
    let result = authenticate("Please authenticate to login").await;
    let _ = result;
}
```

## Configuration

**Android**: Call authentication from a foreground `Activity` context.
**iOS**: Add `NSFaceIDUsageDescription` to your `Info.plist`.
