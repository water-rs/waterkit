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
use waterkit_build::{SwiftBridge, SwiftBridges};

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" || target_os == "macos" {
        SwiftBridges::new()
            .bridge(
                SwiftBridge::new("src/sys/apple/mod.rs")
                    .swift_source("src/sys/apple/Feature.swift")
                    .framework("Foundation"),
            )
            .compile();
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

## Activity results

Android crates that need to launch an activity and await its result can enable
the `activity-result` feature. The feature ships
`waterkit.build.ActivityResultHelper` once through the shared build crate,
declares the `androidx.activity:activity:1.11.0` dependency, and exposes
typed Rust APIs:

```rust,ignore
use waterkit_build::{ResultCode, start_activity_for_result};

let pending = start_activity_for_result(&mut env, &intent)?;
let result = pending.await?;
if result.code() == ResultCode::Ok {
    let data = result.into_data();
}
```

The helper registers an AndroidX activity-result contract on the host
`ComponentActivity`, so host applications do not need request codes,
`onActivityResult` forwarding, or JNI callback glue of their own.

## License

MIT OR Apache-2.0