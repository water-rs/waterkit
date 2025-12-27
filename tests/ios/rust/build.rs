use std::path::PathBuf;

fn main() {
    // let bridge_module = "src/lib.rs";

    // Use waterkit-build to handle Swift bridge generation and compilation
    // We only need generation here if we want to link it in Xcode.
    // But we can also compile it into a static lib.
    // Generate Swift bridge code to a known location for the app
    // Relative to tests/ios/rust/Cargo.toml
    let out_dir = PathBuf::from("../app/WaterKitTest/Generated");
    std::fs::create_dir_all(&out_dir).unwrap();

    let mut bridges = vec!["src/lib.rs".to_string()];

    // Add biometric bridge if feature enabled via env var (set by cargo feature)
    if std::env::var("CARGO_FEATURE_BIOMETRIC").is_ok() {
        bridges.push("../../../biometric/src/sys/apple/mod.rs".to_string());
    }

    // Add sensor bridge if feature enabled
    if std::env::var("CARGO_FEATURE_SENSOR").is_ok() {
        bridges.push("../../../sensor/src/sys/apple/mod.rs".to_string());
    }

    // Add camera bridge if feature enabled
    if std::env::var("CARGO_FEATURE_CAMERA").is_ok() {
        bridges.push("../../../camera/src/sys/apple/mod.rs".to_string());
    }

    // Add location bridge if feature enabled
    if std::env::var("CARGO_FEATURE_LOCATION").is_ok() {
        bridges.push("../../../location/src/sys/apple/mod.rs".to_string());
    }

    // Add permission bridge if feature enabled
    if std::env::var("CARGO_FEATURE_PERMISSION").is_ok() {
        bridges.push("../../../permission/src/sys/apple/mod.rs".to_string());
    }

    // Add notification bridge if feature enabled
    if std::env::var("CARGO_FEATURE_NOTIFICATION").is_ok() {
        bridges.push("../../../notification/src/sys/apple/mod.rs".to_string());
    }

    // Add other crates as needed...

    let bridges_refs: Vec<&str> = bridges.iter().map(|s| s.as_str()).collect();

    waterkit_build::build_apple_bridge(&bridges_refs); // Keeps the cargo rerun logic

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
