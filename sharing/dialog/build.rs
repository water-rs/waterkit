//! Build script for waterkit-dialog.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" {
        use waterkit_build::AppleSwiftConfig;

        let config = AppleSwiftConfig::new("waterkit-dialog", "DialogHelper")
            .swift_source("src/sys/apple/Alert.swift")
            .framework("Foundation")
            .framework("UIKit")
            .framework("PhotosUI")
            .framework("UniformTypeIdentifiers");

        waterkit_build::compile_swift("src/sys/apple/mod.rs", &config);
    }

    if target_os == "android" {
        waterkit_build::build_kotlin(&["src/sys/android/DialogHelper.kt"]);
    }
}
