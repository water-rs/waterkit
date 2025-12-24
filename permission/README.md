# Waterkit Permission

Unified system permission handling.

## Features

- **Unified Enum**: `Permission::Camera`, `Permission::Microphone`, `Permission::Location`, etc.
- **Check Status**: Granted, Denied, Restricted, NotDetermined.
- **Request**: Prompt the user for access.

## Installation

```toml
[dependencies]
waterkit-permission = "0.1"
```

## Supported Permissions

- `Camera`
- `Microphone`
- `Location`
- `PhotoLibrary` (Read/Write)
- `Biometric` (Implicit usually)
- `Notification`

## Usage

```rust
use waterkit_permission::{check, request, Permission, PermissionStatus};

async fn ensure_camera_access() {
    let status = check(Permission::Camera).await;
    
    if status == PermissionStatus::NotDetermined {
        let new_status = request(Permission::Camera).await;
        if new_status == PermissionStatus::Granted {
            println!("Camera access granted!");
        }
    } else if status == PermissionStatus::Granted {
        println!("Already have access.");
    }
}
```

**Note**: You must still add the relevant platform-specific keys to `Info.plist` (iOS/macOS) or `AndroidManifest.xml` (Android) for the permissions you request.
