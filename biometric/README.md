# Waterkit Biometric

Native biometric authentication (TouchID, FaceID, Fingerprint, Face Unlock) for Rust applications.

## Features

- **Unified API**: Single `authenticate` function for all platforms.
- **Native UI**: Uses the system's standard authentication prompts.
- **Fallback Support**: Handles cases where biometrics are unavailable or not enrolled.

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
| **Android** | `androidx.biometric.BiometricPrompt` |
| **Windows** | Windows Hello |
| **Linux** | *Not currently supported* |

## Usage

```rust
use waterkit_biometric::{authenticate, BiometricType};

async fn login() {
    // Optional: Check what type is available
    let bio_type = waterkit_biometric::get_type().await;
    println!("Available biometric: {:?}", bio_type); // e.g., FaceID

    // Authenticate
    let result = authenticate("Please authenticate to login").await;
    
    match result {
        Ok(_) => println!("Success!"),
        Err(e) => println!("Authentication failed: {}", e),
    }
}
```

## Configuration

**Android**: Ensure your activity inherits `FragmentActivity` to support `BiometricPrompt`.
**iOS**: Add `NSFaceIDUsageDescription` to your `Info.plist`.
