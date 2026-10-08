//! Build script for waterkit-share.
//!
//! Emits the framework link flags the Apple backend needs. `cargo` applies
//! them for every Rust-driven link, and `rustc --print=native-static-libs`
//! reports them to embedders that link the crate as a static library.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    match target_os.as_str() {
        "ios" => println!("cargo:rustc-link-lib=framework=UIKit"),
        "macos" => println!("cargo:rustc-link-lib=framework=AppKit"),
        _ => {}
    }
}
