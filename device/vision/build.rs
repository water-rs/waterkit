//! Build script for waterkit-vision.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    let apple = matches!(target_os.as_str(), "ios" | "macos");
    let vision = apple
        && (std::env::var("CARGO_FEATURE_BARCODE").is_ok()
            || std::env::var("CARGO_FEATURE_TEXT").is_ok());
    // `DataScannerViewController` is unavailable on Mac Catalyst, so the
    // scanner bridge only compiles for iOS without the `macabi` ABI.
    let scanner = target_os == "ios"
        && std::env::var("CARGO_CFG_TARGET_ABI").as_deref() != Ok("macabi")
        && std::env::var("CARGO_FEATURE_SCANNER").is_ok();

    if !(vision || scanner) {
        return;
    }

    // Both enabled Apple bridges compile into one static library: a separate
    // compilation per bridge would emit the Swift runtime's `clang_rt` link
    // modifiers twice, and rustc rejects the override.
    let mut bridges = waterkit_build::SwiftBridges::new();
    if vision {
        bridges = bridges.bridge(
            waterkit_build::SwiftBridge::new("src/sys/apple_vision/mod.rs")
                .swift_source("src/sys/apple_vision/Vision.swift")
                .framework("Foundation")
                .framework("CoreVideo")
                .framework("CoreImage")
                .framework("ImageIO")
                .framework("Metal")
                .framework("Vision"),
        );
    }
    if scanner {
        bridges = bridges.bridge(
            waterkit_build::SwiftBridge::new("src/sys/apple/mod.rs")
                .swift_source("src/sys/apple/Scanner.swift")
                .framework("UIKit")
                .framework("Vision")
                .framework("VisionKit"),
        );
    }
    bridges.compile();
}
