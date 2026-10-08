//! Build script for waterkit-vision.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    let apple = matches!(target_os.as_str(), "ios" | "macos");
    let vision = apple
        && (std::env::var("CARGO_FEATURE_BARCODE").is_ok()
            || std::env::var("CARGO_FEATURE_TEXT").is_ok()
            || std::env::var("CARGO_FEATURE_DOCUMENT").is_ok());
    let scanner = target_os == "ios" && std::env::var("CARGO_FEATURE_SCANNER").is_ok();

    if !(vision || scanner) {
        return;
    }

    // Both enabled Apple bridges compile into one static library: separate
    // `compile_swift`/`build_apple_bridge` calls would each emit the Swift
    // runtime's `clang_rt` link modifiers, and rustc rejects the override.
    let mut crates = Vec::new();
    if vision {
        crates.push(
            waterkit_build::SwiftBridgeCrate::new("src/sys/apple_vision/mod.rs")
                .swift_source("src/sys/apple_vision/Vision.swift")
                .framework("CoreVideo")
                .framework("CoreImage")
                .framework("DataDetection")
                .framework("ImageIO")
                .framework("Metal")
                .framework("Vision"),
        );
    }
    if scanner {
        crates.push(
            waterkit_build::SwiftBridgeCrate::new("src/sys/apple/mod.rs")
                .swift_source("src/sys/apple/Scanner.swift")
                .framework("UIKit")
                .framework("Vision")
                .framework("VisionKit"),
        );
    }
    waterkit_build::compile_multi_swift("waterkit_vision_swift_bridge", crates);
}
