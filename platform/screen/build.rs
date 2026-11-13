//! The `display_stream` test target probes `WindowServer` through the private
//! `SkyLight` framework, which only the SDK's private framework directory
//! lists. Only test targets get the search path; the library never links it.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg-tests=-F/System/Library/PrivateFrameworks");
    }
}
