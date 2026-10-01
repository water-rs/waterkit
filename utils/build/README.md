# waterkit-build

Shared build utilities for waterkit crates.

## Features

- **Apple**: Swift bridge generation and Swift source compilation
- **Android**: `DexHelper`/`dex_helper!` resolve helper classes through the
  application's `ClassLoader`. Crates declare their Kotlin sources in
  `[package.metadata.waterui.android]` (`kotlin-sources`, `required-feature`)
  and the packager compiles them into the app's own DEX — nothing is compiled
  or dexed at build-script time.

## Usage

In your `build.rs`:

```rust
use waterkit_build::build_apple_bridge;

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" || target_os == "macos" {
        build_apple_bridge(&["src/sys/apple/mod.rs"]);
    }
}
```

In `Cargo.toml`:

```toml
[package.metadata.waterui.android]
kotlin-sources = ["src/sys/android/Helper.kt"]
```

In the Android sys module:

```rust
use waterkit_build::{DexHelper, dex_helper};

static HELPER: DexHelper = dex_helper!("com.example.Helper");
```

## License

MIT OR Apache-2.0
