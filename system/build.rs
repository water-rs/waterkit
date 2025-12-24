//! Build script for waterkit-system.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" || target_os == "macos" {
        use waterkit_build::AppleSwiftConfig;

        let target = std::env::var("TARGET").unwrap();
        let mut config = AppleSwiftConfig::new("waterkit-system", "SystemHelper")
            .swift_source("src/sys/apple/System.swift")
            .framework("Foundation")
            .framework("Network");

        if target.contains("ios") {
            config = config.framework("UIKit");
        } else {
            config = config.framework("AppKit");
        }

        waterkit_build::compile_swift("src/sys/apple/mod.rs", &config);
    }

    if target_os == "android" {
        waterkit_build::build_kotlin(&["src/sys/android/SystemHelper.kt"]);
    }
}
