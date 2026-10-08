//! Build script for waterkit-dialog.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" {
        waterkit_build::SwiftBridges::new()
            .bridge(
                waterkit_build::SwiftBridge::new("src/sys/apple/mod.rs")
                    .swift_source("src/sys/apple/Alert.swift")
                    .framework("Foundation")
                    .framework("UIKit")
                    .framework("PhotosUI")
                    .framework("UniformTypeIdentifiers"),
            )
            .compile();
    }
}
