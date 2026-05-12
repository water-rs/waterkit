//! Build script for waterkit-video.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" || target_os == "macos" {
        use waterkit_build::AppleSwiftConfig;

        let target = std::env::var("TARGET").unwrap();
        let mut config = AppleSwiftConfig::new("waterkit-video", "PictureInPictureHelper")
            .swift_source("src/sys/apple/PictureInPictureHelper.swift")
            .framework("Foundation")
            .framework("AVFoundation")
            .framework("AVKit")
            .framework("CoreMedia")
            .framework("CoreVideo")
            .framework("Metal");

        if target.contains("ios") {
            config = config.framework("UIKit");
        } else {
            config = config.framework("AppKit");
        }

        waterkit_build::compile_swift("src/sys/apple/bridge.rs", &config);
    }

    if target_os == "android" {
        waterkit_build::build_kotlin(&["src/sys/android/PictureInPictureHelper.kt"]);
    }
}
