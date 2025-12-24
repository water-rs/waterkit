//! Build script for waterkit-clipboard.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" || target_os == "macos" {
        use waterkit_build::AppleSwiftConfig;

        let config = AppleSwiftConfig::new("waterkit-clipboard", "ClipboardHelper")
            .swift_source("src/sys/apple/clipboard.swift")
            .framework("Foundation");

        #[cfg(target_os = "ios")]
        let config = config.framework("UIKit");

        #[cfg(target_os = "macos")]
        let config = config.framework("AppKit");

        waterkit_build::compile_swift("src/sys/apple/mod.rs", &config);
    }

    if target_os == "android" {
        waterkit_build::build_kotlin(&["src/sys/android/ClipboardHelper.kt"]);
    }
}
