use std::path::PathBuf;

fn main() {
    let bridge_module = "src/lib.rs";
    
    // Use waterkit-build to handle Swift bridge generation and compilation
    // We only need generation here if we want to link it in Xcode.
    // But we can also compile it into a static lib.
    waterkit_build::build_apple_bridge(&[bridge_module]);
    
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/lib.rs");
}
