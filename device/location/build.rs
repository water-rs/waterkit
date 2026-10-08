//! Build script for waterkit-location.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" || target_os == "macos" {
        // Compile the Swift implementations + swift-bridge glue into a static library and
        // link it so downstream consumers don't need to manually add Swift sources.
        waterkit_build::SwiftBridges::new()
            .bridge(
                waterkit_build::SwiftBridge::new("src/sys/apple/mod.rs")
                    .swift_source("src/sys/apple/Location.swift")
                    .framework("Foundation")
                    .framework("CoreLocation"),
            )
            .compile();
    }
}
