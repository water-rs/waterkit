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

### 1. Build for iOS Simulator

Use the `waterkit-test` CLI:

```bash
# From workspace root
cargo run -p waterkit-test -- ios biometric
```

This will:
1. Update `tests/ios/rust/Cargo.toml` to point to the `biometric` crate.
2. Build the Rust static library for `aarch64-apple-ios-sim`.

### 2. Run the App

1. Open `tests/ios/app/Package.swift` in Xcode.
2. Ensure the destination is an iOS Simulator.
3. Link the Rust library (will need manual linking in Xcode for now as it's a static lib).
4. Run!

## Requirements

- Xcode 15+
- Rust with `aarch64-apple-ios-sim` target.
