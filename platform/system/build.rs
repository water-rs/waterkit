//! Build script for waterkit-system.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" || target_os == "macos" {
        use waterkit_build::{SwiftBridge, SwiftBridges};

        let mut bridge = SwiftBridge::new("src/sys/apple/mod.rs")
            .swift_source("src/sys/apple/System.swift")
            .framework("Foundation")
            .framework("Network");

        if target_os == "ios" {
            bridge = bridge.framework("UIKit");
        } else {
            bridge = bridge.framework("AppKit");
        }

        SwiftBridges::new().bridge(bridge).compile();
    }
}
