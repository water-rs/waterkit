//! Build script for waterkit-sensor.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" || target_os == "macos" {
        let mut bridge = waterkit_build::SwiftBridge::new("src/sys/apple/mod.rs")
            .swift_source("src/sys/apple/sensor.swift")
            .framework("Foundation");

        if target_os == "ios" {
            bridge = bridge.framework("CoreMotion");
        } else {
            bridge = bridge.framework("IOKit");
        }

        waterkit_build::SwiftBridges::new().bridge(bridge).compile();
    }
}
