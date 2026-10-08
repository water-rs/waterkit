//! Build script for waterkit-codec.

// Nested cfg_aliases expand recursively.
#![recursion_limit = "2048"]

fn main() {
    compose_yuv_shader();

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
        waterkit_build::SwiftBridges::new()
            .bridge(
                waterkit_build::SwiftBridge::new("src/image_apple.rs")
                    .swift_source("src/sys/apple/ImageDecoder.swift")
                    .framework("Foundation")
                    .framework("CoreGraphics")
                    .framework("ImageIO")
                    .framework("CoreImage")
                    .framework("VideoToolbox"),
            )
            .compile();
    }
}

/// Composes the YUV-to-RGBA shader, the codec's source after
/// waterkit-video-core's shared YCbCr fragment, into `OUT_DIR` for
/// `YUV_COLOR_SHADER_WGSL`, and with the `gpu` feature compiles it to the
/// target's native shader format.
fn compose_yuv_shader() {
    use std::io::Write as _;

    const SOURCE: &str = "src/yuv_to_rgba.wgsl";
    writeln!(std::io::stdout().lock(), "cargo:rerun-if-changed={SOURCE}")
        .expect("failed to emit the shader source tracking directive");
    let codec = std::fs::read_to_string(SOURCE)
        .unwrap_or_else(|error| panic!("failed to read {SOURCE}: {error}"));
    let composed = format!("{}{codec}", waterkit_video_core::YCBCR_WGSL);
    let out_dir = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR for build scripts");
    std::fs::write(
        std::path::Path::new(&out_dir).join("yuv_color_source.wgsl"),
        &composed,
    )
    .expect("failed to write the composed YUV shader");

    #[cfg(feature = "gpu")]
    shaderloom::build::compile_wgsl_source(SOURCE, &composed, "yuv_color");
}
