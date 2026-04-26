//! Build script for waterkit-codec.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "ios" || target_os == "macos" {
        let config = waterkit_build::AppleSwiftConfig::new("waterkit-codec", "CodecImageHelper")
            .swift_source("src/sys/apple/ImageDecoder.swift")
            .framework("Foundation")
            .framework("CoreGraphics")
            .framework("ImageIO")
            .framework("CoreImage")
            .framework("VideoToolbox");

        waterkit_build::compile_swift("src/image_apple.rs", &config);
    }
}
