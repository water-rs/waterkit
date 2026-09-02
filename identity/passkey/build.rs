//! Build script for waterkit-passkey.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" || target_os == "macos" {
        use waterkit_build::AppleSwiftConfig;

        let mut config = AppleSwiftConfig::new("waterkit-passkey", "PasskeyHelper")
            .swift_source("src/sys/apple/Passkey.swift")
            .framework("Foundation")
            .framework("AuthenticationServices");

        if target_os == "ios" {
            config = config.framework("UIKit");
        }

        if target_os == "macos" {
            config = config.framework("AppKit");
        }

        waterkit_build::compile_swift("src/sys/apple/mod.rs", &config);
    }

    if target_os == "android" {
        waterkit_build::build_kotlin(&["src/sys/android/PasskeyHelper.kt"]);
    }
}
