//! Build script for waterkit-clipboard.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    // iOS uses Swift bridge (macOS uses clipboard-rs)
    if target_os == "ios" {
        use waterkit_build::AppleSwiftConfig;

        let config = AppleSwiftConfig::new("waterkit-clipboard", "ClipboardHelper")
            .swift_source("src/sys/apple/clipboard.swift")
            .framework("Foundation")
            .framework("UIKit")
            .framework("UniformTypeIdentifiers")
            .framework("MobileCoreServices");

        waterkit_build::compile_swift("src/sys/apple/mod.rs", &config);
    }

    if target_os == "android" {
        waterkit_build::build_kotlin(&["src/sys/android/ClipboardHelper.kt"]);
    }
}
