# Waterkit FS

File system utilities and path management for cross-platform apps.

## Features

- **Standard Paths**: Easy access to `Documents`, `Cache`, `Temporary` directories on all platforms.
- **Sandboxing**: Handles mobile sandbox constraints (iOS/Android).
- **Helpers**: Common file operations.

## Installation

```toml
[dependencies]
waterkit-fs = "0.1"
```

## Platform Support

| Platform | Implementation |
| :--- | :--- |
| **iOS** | `FileManager.default.urls` |
| **Android** | `Context.getFilesDir()`, `getCacheDir()` |
| **Desktop** | `dirs` crate |

## Usage

```rust
use waterkit_fs::{get_documents_dir, get_cache_dir};

fn paths() {
    if let Some(docs) = get_documents_dir() {
        println!("Documents: {:?}", docs);
        // On iOS: /var/mobile/Containers/Data/Application/.../Documents
    }
    
    if let Some(cache) = get_cache_dir() {
        println!("Cache: {:?}", cache);
    }
}
```
