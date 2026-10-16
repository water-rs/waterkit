//! Build script for waterkit-store.
//!
//! Compiles the `StoreKit` 2 bridge when targeting Apple platforms.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    if target_os == "ios" || target_os == "macos" {
        waterkit_build::SwiftBridges::new()
            .bridge(
                waterkit_build::SwiftBridge::new("src/sys/apple/bridge.rs")
                    .swift_source("src/sys/apple/StoreKit.swift")
                    .framework("StoreKit"),
            )
            .compile();
    }
}
