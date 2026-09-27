//! Build script for waterkit-codec.

// Nested cfg_aliases expand recursively.
#![recursion_limit = "2048"]

fn main() {
    // The YUV-to-RGBA compute shader ships pre-translated under
    // src/shaders/compiled (regenerate with package-shaders.sh), so the host
    // runs no `naga`; only xcrun/dxc run on Apple/Windows targets.
    #[cfg(feature = "gpu")]
    {
        const PACKAGED_SHADERS: &str = "src/shaders/compiled";
        if std::env::var("CARGO_CFG_TARGET_VENDOR").as_deref() == Ok("apple") {
            shaderloom::packaged::compile_packaged_metallib(PACKAGED_SHADERS, "yuv_color");
        }
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            shaderloom::packaged::compile_packaged_dxil(PACKAGED_SHADERS, "yuv_color");
        }
    }

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
            feature = "software-decode",
            not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
        ) },
        waterkit_av1_software_encode: { all(
            feature = "software-encode",
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
                any(feature = "software-decode", feature = "software-encode"),
                not(any(target_os = "ios", target_os = "android", target_arch = "wasm32"))
            )
        ) },
        waterkit_software_frames: { any(
            target_os = "android",
            target_os = "windows",
            all(target_os = "linux", feature = "vaapi"),
            all(
                any(feature = "software-decode", feature = "software-encode"),
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
