//! Build script for waterkit-bluetooth.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" || target_os == "macos" {
        let mut bridge = waterkit_build::SwiftBridge::new("src/sys/apple/mod.rs")
            .swift_source("src/sys/apple/Bluetooth.swift")
            .framework("Foundation")
            .framework("CoreBluetooth");

        if target_os == "macos" {
            bridge = bridge.framework("IOBluetooth");
        }

        waterkit_build::SwiftBridges::new().bridge(bridge).compile();
    }
}
