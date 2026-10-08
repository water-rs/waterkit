//! Build script for waterkit-language.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" || target_os == "macos" {
        waterkit_build::SwiftBridges::new()
            .bridge(
                waterkit_build::SwiftBridge::new("src/sys/apple/mod.rs")
                    .swift_source("src/sys/apple/Translation.swift")
                    .framework("Foundation")
                    .framework("Translation"),
            )
            .compile();
    }
}
