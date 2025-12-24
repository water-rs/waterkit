//! Build script for waterkit-location.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" || target_os == "macos" {
        waterkit_build::build_apple_bridge(&["src/sys/apple/mod.rs"]);
    }

    if target_os == "android" {
        waterkit_build::build_kotlin(&["src/sys/android/LocationHelper.kt"]);
    }
}
