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

### 1. Build Rust libraries

```bash
# From workspace root
cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 -o tests/android/app/src/main/jniLibs build --release -p waterkit-android-test
```

### 2. Build and run Android app

```bash
cd tests/android
./gradlew installDebug
adb shell am start -n com.waterkit.test/.MainActivity
```

## Adding new crates to test

1. Add dependency in `rust/Cargo.toml`
2. Add JNI functions in `rust/src/lib.rs`
3. Add UI buttons in `app/.../MainActivity.kt`

## Requirements

- Android SDK with platform 34
- Android NDK
- `cargo-ndk` (`cargo install cargo-ndk`)
- Kotlin 1.9+
