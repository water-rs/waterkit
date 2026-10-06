//! Build script for waterkit-vision.

fn main() {
    waterkit_build::build_apple_bridge(["src/sys/apple/mod.rs"]);
}
