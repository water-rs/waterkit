//! Build script for waterkit-haptic.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" || target_os == "macos" {
        use waterkit_build::{SwiftBridge, SwiftBridges};

        let mut bridge = SwiftBridge::new("src/sys/apple/mod.rs")
            .swift_source("src/sys/apple/Haptic.swift")
            .framework("Foundation");

        if target_os == "ios" {
            bridge = bridge.framework("UIKit").framework("CoreHaptics");
        } else {
            bridge = bridge.framework("AppKit");
        }

        SwiftBridges::new().bridge(bridge).compile();
    }
}
