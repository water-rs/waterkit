# waterkit-build

Shared build utilities for waterkit crates.

## Features

- **Apple**: Swift bridge generation and Swift source compilation
- **Android**: Kotlin → DEX compilation for embedding in Rust binaries

## Usage

In your `build.rs`:

```rust
use waterkit_build::{build_apple_bridge, build_kotlin};

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" || target_os == "macos" {
        build_apple_bridge(&["src/sys/apple/mod.rs"]);
    }

    if target_os == "android" {
        build_kotlin(&["src/sys/android/Helper.kt"]);
    }
}
```

## License

MIT OR Apache-2.0
