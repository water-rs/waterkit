//! Build script for waterkit-fs.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" || target_os == "macos" {
        waterkit_build::build_apple_bridge(["src/sys/apple/mod.rs"]);
    }
}
