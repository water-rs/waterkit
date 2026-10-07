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
cargo run -p waterkit-test -- android .
```

Passing the workspace root (`.`) enables every supported Android harness
feature in one APK so they can be exercised together on a connected device.

Add `--interactive` to run cases that need someone to use Android's photo and
document pickers. These cases run in this order:

1. `dialog.photo_picker`
2. `dialog.file_picker`
3. `dialog.file_picker_multiple`
4. `dialog.picker_cancelled`

Before starting an interactive run, create these two fixture files on the
device. Their contents must match exactly:

```bash
adb shell "mkdir -p /sdcard/Download"
adb shell "printf 'waterkit activity results a' > /sdcard/Download/waterkit-activity-result-a.txt"
adb shell "printf 'waterkit activity results b' > /sdcard/Download/waterkit-activity-result-b.txt"
```

Place a PNG in `/sdcard/Pictures` for `dialog.photo_picker`. For example,
capture the emulator display and scan the image into Android's media library:

```bash
adb exec-out screencap -p > /tmp/waterkit-activity-result.png
adb push /tmp/waterkit-activity-result.png /sdcard/Pictures/waterkit-activity-result.png
adb shell am broadcast -a android.intent.action.MEDIA_SCANNER_SCAN_FILE \
  -d file:///sdcard/Pictures/waterkit-activity-result.png
```

Run the interactive cases with:

```bash
cargo run -p waterkit-test -- android sharing/dialog --interactive
```

Choose the PNG, choose fixture A in the single-file picker, select both
fixtures in the multiple-file picker, then press Back in the final picker.
Interactive runs allow up to ten minutes for the report. Fixture files and
image binaries are local test data and are not committed to the repository.

The Android app also keeps the manual UI buttons for local exploration. The
OTP section can start addressed SMS and User Consent requests; do not run
either manual request while the native test report is running. The driver
enables host SMS delivery only when `ro.boot.qemu` reports `1`; physical
devices skip addressed-message delivery.

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