//! Build script for waterkit-clipboard.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    // iOS uses Swift bridge (macOS uses clipboard-rs)
    if target_os == "ios" {
        waterkit_build::SwiftBridges::new()
            .bridge(
                waterkit_build::SwiftBridge::new("src/sys/apple/mod.rs")
                    .swift_source("src/sys/apple/clipboard.swift")
                    .framework("Foundation")
                    .framework("UIKit")
                    .framework("UniformTypeIdentifiers")
                    .framework("MobileCoreServices"),
            )
            .compile();
    }
}
