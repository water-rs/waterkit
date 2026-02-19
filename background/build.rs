//! Build script for waterkit-background.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" {
        use waterkit_build::AppleSwiftConfig;

        let config = AppleSwiftConfig::new("waterkit-background", "BackgroundHelper")
            .swift_source("src/sys/apple/Background.swift")
            .framework("Foundation")
            .framework("BackgroundTasks")
            .framework("UIKit");

        waterkit_build::compile_swift("src/sys/apple/mod.rs", &config);
    }
}
