# Android Test Framework

Reusable Android test harness for waterkit crates.

## Structure

```
tests/android/
├── app/                    # Android app module
│   ├── build.gradle.kts
│   └── src/main/
│       ├── AndroidManifest.xml
│       ├── kotlin/         # Kotlin test UI
│       └── res/
├── rust/                   # Test JNI library
│   ├── Cargo.toml
│   └── src/lib.rs
├── build.gradle.kts
├── settings.gradle.kts
└── README.md
```

## Usage

Run through the `waterkit-test` CLI from the workspace root. The command builds
the selected Rust feature library for Android, builds the APK, installs it on
the connected device or emulator, launches the app with `run_test=true`, pulls
the structured JSON report from app storage, and fails if any reported case
failed.

```bash
cargo run -p waterkit-test -- android device/sensor
```

The Android app also keeps the manual UI buttons for local exploration.

## Adding new crates to test

1. Add the feature mapping in `rust/Cargo.toml`.
2. Add structured cases in `rust/src/lib.rs`.
3. Add UI buttons in `app/.../MainActivity.kt` only when the crate needs
   manual interaction.

## Requirements

- Android SDK with platform 34
- Android NDK
- `cargo-ndk` (`cargo install cargo-ndk`)
- Kotlin 1.9+
