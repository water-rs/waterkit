//! Build script for waterkit-codec.

// Nested cfg_aliases expand recursively.
#![recursion_limit = "2048"]

fn main() {
    // The YUV-to-RGBA compute shader only exists on the GPU texture-output path.
    #[cfg(feature = "gpu")]
    shaderloom::build::compile_wgsl_shader("src/yuv_to_rgba.wgsl", "yuv_color");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // Codec backend predicates, defined once and emitted as cfg aliases.
    // hw_* are the four hardware backends; *_av1_software the CPU fallback;
    // *_hw_codec any hardware backend; *_any_codec any backend at all;
    // *_software_frames: decoded frames stored as CPU planes (every backend
    // except Apple's `IOSurface` path).
    cfg_aliases::cfg_aliases! {
        waterkit_hw_codec_apple: { target_vendor = "apple" },
        waterkit_hw_codec_android: { target_os = "android" },
        waterkit_hw_codec_windows: { target_os = "windows" },
        waterkit_hw_codec_vaapi: { all(target_os = "linux", feature = "vaapi") },
        waterkit_av1_software: { all(
            feature = "software-fallback",
            not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
        ) },
        waterkit_hw_codec: { any(
            target_vendor = "apple",
            target_os = "android",
            target_os = "windows",
            all(target_os = "linux", feature = "vaapi")
        ) },
        waterkit_any_codec: { any(
            target_vendor = "apple",
            target_os = "android",
            target_os = "windows",
            all(target_os = "linux", feature = "vaapi"),
            all(
                feature = "software-fallback",
                not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
            )
        ) },
        waterkit_software_frames: { any(
            target_os = "android",
            target_os = "windows",
            all(target_os = "linux", feature = "vaapi"),
            all(
                feature = "software-fallback",
                not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
            )
        ) },
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
