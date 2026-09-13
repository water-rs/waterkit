//! Build script for waterkit-test-ios.
//!
//! Generates Swift bridge code for iOS app integration.
//! Only runs on macOS host when targeting Apple platforms.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    if target_os != "macos" && target_os != "ios" {
        return;
    }

    // swift-bridge-build is only available on macOS host
    #[cfg(target_os = "macos")]
    apple_build();
}

#[cfg(target_os = "macos")]
fn apple_build() {
    use std::path::PathBuf;

    // Only this crate's own bridge is generated into the app: each waterkit
    // component crate already compiles its `sys/apple` Swift bridge into its
    // own static lib via `build_apple_bridge`, and those objects are bundled
    // into `libwaterkit_test_ios.a`, so the final link resolves every
    // `extern "Swift"` symbol without the app ever seeing the feature bridges.
    // Relative to tests/ios/rust/Cargo.toml
    let out_dir = PathBuf::from("../app/WaterKitTest/Generated");
    std::fs::create_dir_all(&out_dir).unwrap();

    let bridges = vec!["src/lib.rs".to_string()];

    let bridges_refs: Vec<&str> = bridges.iter().map(|s| s.as_str()).collect();

    waterkit_build::build_apple_bridge(bridges_refs); // Keeps the cargo rerun logic

    // Manual generation to the specific path
    swift_bridge_build::parse_bridges(bridges)
        .write_all_concatenated(out_dir.clone(), env!("CARGO_PKG_NAME"));

    // Generate Bridging-Header.h
    let pkg_name = env!("CARGO_PKG_NAME");
    let bridging_header = format!(
        "#include \"SwiftBridgeCore.h\"\n#include \"{}/{}.h\"\n",
        pkg_name, pkg_name
    );
    std::fs::write(out_dir.join("Bridging-Header.h"), bridging_header).unwrap();

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/lib.rs");
}
