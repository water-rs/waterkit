//! Build script for waterkit-background.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" {
        waterkit_build::SwiftBridges::new()
            .bridge(
                waterkit_build::SwiftBridge::new("src/sys/apple/mod.rs")
                    .swift_source("src/sys/apple/Background.swift")
                    .framework("Foundation")
                    .framework("BackgroundTasks")
                    .framework("UIKit"),
            )
            .compile();
    }
}
