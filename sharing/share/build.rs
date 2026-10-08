//! Build script for waterkit-share.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" || target_os == "macos" {
        let mut bridge = waterkit_build::SwiftBridge::new("src/sys/apple/mod.rs")
            .swift_source("src/sys/apple/Share.swift")
            .framework("Foundation");

        if target_os == "ios" {
            bridge = bridge.framework("UIKit");
        } else {
            bridge = bridge.framework("AppKit");
        }

        waterkit_build::SwiftBridges::new().bridge(bridge).compile();
    }
}
