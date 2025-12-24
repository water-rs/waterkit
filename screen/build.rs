//! Build script for waterkit-screen.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" || target_os == "macos" {
        use waterkit_build::AppleSwiftConfig;

        let target = std::env::var("TARGET").unwrap();

        let swift_source = if target_os == "macos" {
            "src/platform/apple/ScreenMacOS.swift"
        } else {
            "src/platform/apple/Screen.swift"
        };

        let mut config = AppleSwiftConfig::new("waterkit-screen", "ScreenHelper")
            .swift_source(swift_source)
            .framework("Foundation");

        if target.contains("ios") {
            config = config.framework("UIKit");
        } else {
            config = config.framework("Cocoa").framework("ScreenCaptureKit");
        }

        waterkit_build::compile_swift("src/platform/apple.rs", &config);

        // Swift runtime for async/await
        if target_os == "macos" {
            println!("cargo:rustc-link-arg=-rpath");
            println!("cargo:rustc-link-arg=/usr/lib/swift");
        }
    }
}
