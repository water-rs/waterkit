//! Build script for waterkit-test-ios.
//!
//! Generates Swift bridge code for iOS app integration.
//! Only runs on macOS host when targeting Apple platforms.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    if target_os == "macos" || target_os == "ios" {
        // swift-bridge-build is only available on macOS host
        #[cfg(target_os = "macos")]
        apple_build();
    }
}

#[cfg(target_os = "macos")]
fn apple_build() {
    // Only this crate's own bridge is generated into the app: each waterkit
    // component crate already compiles its `sys/apple` Swift bridge into its
    // own static lib via `SwiftBridges`, and those objects are bundled
    // into `libwaterkit_test_ios.a`, so the final link resolves every
    // `extern "Swift"` symbol without the app ever seeing the feature bridges.
    // Relative to tests/ios/rust/Cargo.toml
    waterkit_build::SwiftBridges::new()
        .bridge(waterkit_build::SwiftBridge::new("src/lib.rs"))
        .generate_into("../app/WaterKitTest/Generated");
}
