//! Build script for waterkit-screen.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" || target_os == "macos" {
        use waterkit_build::{SwiftBridge, SwiftBridges};

        let swift_source = if target_os == "macos" {
            "src/sys/apple/ScreenMacOS.swift"
        } else {
            "src/sys/apple/Screen.swift"
        };

        let mut bridge = SwiftBridge::new("src/sys/apple/mod.rs")
            .swift_source(swift_source)
            .framework("Foundation");

        if target_os == "ios" {
            bridge = bridge.framework("UIKit");
        } else {
            bridge = bridge
                .framework("Cocoa")
                .framework("ScreenCaptureKit")
                .framework("IOKit");
        }

        SwiftBridges::new().bridge(bridge).compile();
    }
}
