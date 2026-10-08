//! Build script for waterkit-audio.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    let media_session_enabled = std::env::var_os("CARGO_FEATURE_MEDIA_SESSION").is_some();

    if media_session_enabled && (target_os == "ios" || target_os == "macos") {
        use waterkit_build::{SwiftBridge, SwiftBridges};

        let mut bridge = SwiftBridge::new("src/sys/apple/mod.rs")
            .swift_source("src/sys/apple/MediaHelper.swift")
            .swift_source("src/sys/apple/AudioPlayerHelper.swift")
            .framework("Foundation")
            .framework("MediaPlayer")
            .framework("AVFoundation");

        if target_os == "ios" {
            bridge = bridge.framework("UIKit");
        } else {
            bridge = bridge.framework("AppKit");
        }

        SwiftBridges::new().bridge(bridge).compile();
    }
}
