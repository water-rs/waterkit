//! Build script for waterkit-codec.

fn main() {
    // The YUV-to-RGBA compute shader only exists on the GPU texture-output path.
    #[cfg(feature = "gpu")]
    shaderloom::build::compile_wgsl_shader("src/yuv_to_rgba.wgsl", "yuv_color");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_vendor = std::env::var("CARGO_CFG_TARGET_VENDOR").unwrap_or_default();
    let software_frames = target_vendor != "apple"
        || (!matches!(target_os.as_str(), "ios" | "tvos" | "watchos")
            && std::env::var_os("CARGO_FEATURE_SOFTWARE_FALLBACK").is_some());
    println!("cargo:rustc-check-cfg=cfg(waterkit_software_frames)");
    if software_frames {
        println!("cargo:rustc-cfg=waterkit_software_frames");
    }

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
