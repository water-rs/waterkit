//! Build script for waterkit-vision.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    let scanner = std::env::var("CARGO_FEATURE_SCANNER").is_ok();

    if target_os == "ios" && scanner {
        use waterkit_build::AppleSwiftConfig;

        let config = AppleSwiftConfig::new("waterkit-vision", "ScannerHelper")
            .swift_source("src/sys/apple/Scanner.swift")
            .framework("Foundation")
            .framework("UIKit")
            .framework("Vision")
            .framework("VisionKit");

        waterkit_build::compile_swift("src/sys/apple/mod.rs", &config);
    }
}
