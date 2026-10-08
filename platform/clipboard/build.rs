//! Build script for waterkit-clipboard.
//!
//! Emits the framework link flags the Apple backend needs. `cargo` applies
//! them for every Rust-driven link, and `rustc --print=native-static-libs`
//! reports them to embedders that link the crate as a static library.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" {
        println!("cargo:rustc-link-lib=framework=UIKit");
        println!("cargo:rustc-link-lib=framework=UniformTypeIdentifiers");
        println!("cargo:rustc-link-lib=framework=MobileCoreServices");
    }
}
