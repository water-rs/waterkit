//! Build script for waterkit-location-test.

use std::path::PathBuf;
use waterkit_build::{compile_multi_swift, SwiftBridgeCrate};

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    if target_os != "macos" && target_os != "ios" {
        return;
    }

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let location = manifest_dir.join("../../../location");
    let permission = manifest_dir.join("../../../permission");

    compile_multi_swift(
        "LocationTest",
        [
            SwiftBridgeCrate::new(location.join("src/sys/apple/mod.rs"))
                .swift_source(location.join("src/sys/apple/Location.swift"))
                .framework("CoreLocation"),
            SwiftBridgeCrate::new(permission.join("src/sys/apple/mod.rs"))
                .swift_source(permission.join("src/sys/apple/Permission.swift"))
                .framework("CoreLocation")
                .framework("AVFoundation")
                .framework("Photos")
                .framework("Contacts")
                .framework("EventKit"),
        ],
    );
}
