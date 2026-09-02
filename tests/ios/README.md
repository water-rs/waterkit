# iOS Test Framework

Minimal iOS test harness for waterkit crates.

## Structure

```
tests/ios/
├── app/            # SwiftUI App (Swift Package)
│   ├── Package.swift
│   └── WaterKitTest/
│       ├── WaterKitTestApp.swift
│       └── ContentView.swift
└── rust/           # Rust Bridge
    ├── Cargo.toml
    ├── build.rs
    └── src/lib.rs
```

## Usage

Boot an iOS Simulator, then run through the `waterkit-test` CLI from the
workspace root. The command builds the selected Rust feature library, compiles
the SwiftUI harness, installs and launches it on the booted simulator, reads the
structured JSON report from the app container, and fails if any reported case
failed.

```bash
cargo run -p waterkit-test -- ios device/sensor
```

The SwiftUI app still exposes a manual "Run All Tests" button for local
exploration.

## Requirements

- Xcode 15+
- Rust with `aarch64-apple-ios-sim` target.
