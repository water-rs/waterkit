//! Build script for waterkit-passkey.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" || target_os == "macos" {
        use waterkit_build::{SwiftBridge, SwiftBridges};

        let mut bridge = SwiftBridge::new("src/sys/apple/mod.rs")
            .swift_source("src/sys/apple/Passkey.swift")
            .framework("Foundation")
            .framework("AuthenticationServices");

        if target_os == "ios" {
            bridge = bridge.framework("UIKit");
        }

        if target_os == "macos" {
            bridge = bridge.framework("AppKit");
        }

        SwiftBridges::new().bridge(bridge).compile();
    }
}
