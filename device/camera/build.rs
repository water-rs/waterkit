//! Build script for waterkit-camera.

use std::io::Write as _;

/// The frame converter's shared shader part, composed before every entry.
const CONVERTER_COMMON: &str = "src/shaders/frame_converter.wgsl";

/// One converter entry point: its source, the artifact it compiles to, and
/// whether it needs the YCbCr fragment from `waterkit-video-core`.
const CONVERTER_ENTRIES: [(&str, &str, bool); 3] = [
    ("src/shaders/convert_rgb.wgsl", "frame_convert_rgb", false),
    (
        "src/shaders/convert_ycbcr420.wgsl",
        "frame_convert_ycbcr420",
        true,
    ),
    (
        "src/shaders/convert_ycbcr422.wgsl",
        "frame_convert_ycbcr422",
        true,
    ),
];

fn main() {
    compile_frame_converter();

    #[cfg(feature = "preview-example")]
    shaderloom::build::compile_wgsl_shader("examples/preview.wgsl", "camera_preview");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" || target_os == "macos" {
        use waterkit_build::{SwiftBridge, SwiftBridges};

        let mut bridge = SwiftBridge::new("src/sys/apple/mod.rs")
            .swift_source("src/sys/apple/CameraHelper.swift")
            .framework("Foundation")
            .framework("AVFoundation")
            .framework("CoreMedia")
            .framework("CoreVideo");

        if target_os == "ios" {
            bridge = bridge.framework("UIKit");
        } else {
            bridge = bridge.framework("AppKit");
        }

        SwiftBridges::new().bridge(bridge).compile();
    }
}

/// Composes each converter module from its parts and compiles it with
/// shaderloom, so the shipped artifacts are the target's native shaders.
fn compile_frame_converter() {
    let common = read_tracked(CONVERTER_COMMON);
    for (path, artifact, uses_ycbcr) in CONVERTER_ENTRIES {
        let entry = read_tracked(path);
        let prelude = if uses_ycbcr {
            waterkit_video_core::YCBCR_WGSL
        } else {
            ""
        };
        shaderloom::build::compile_wgsl_source(
            path,
            &format!("{prelude}{common}{entry}"),
            artifact,
        );
    }
}

fn read_tracked(path: &str) -> String {
    writeln!(std::io::stdout().lock(), "cargo:rerun-if-changed={path}")
        .expect("failed to emit the shader source tracking directive");
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("failed to read {path}: {error}"))
}
