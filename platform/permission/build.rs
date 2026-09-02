//! Build script for waterkit-permission.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" || target_os == "macos" {
        // Compile the Swift implementations + swift-bridge glue into a static library and
        // link it, so downstream consumers (e.g. Rust staticlibs embedded in Xcode projects)
        // don't need to manually add Swift sources just to satisfy `extern "Swift"` symbols.
        let pkg_name = std::env::var("CARGO_PKG_NAME").unwrap();
        let config = waterkit_build::AppleSwiftConfig::new(pkg_name, "waterkit_permission_swift")
            .swift_source("src/sys/apple/Permission.swift")
            .framework("CoreLocation")
            .framework("AVFoundation")
            .framework("Photos")
            .framework("Contacts")
            .framework("EventKit");

        waterkit_build::compile_swift("src/sys/apple/mod.rs", &config);
    }

    if target_os == "android" {
        waterkit_build::build_kotlin(&["src/sys/android/PermissionHelper.kt"]);
    }
}
