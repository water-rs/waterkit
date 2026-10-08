//! Link the Apple frameworks the implementation calls. When the crate is
//! packaged as a static archive the directives only reach cargo-driven
//! links; Xcode projects consuming the archive must list the frameworks
//! themselves (as the test harness does).

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    match target_os.as_str() {
        // UIApplication + UIScene (iOS and Mac Catalyst).
        "ios" => println!("cargo:rustc-link-lib=framework=UIKit"),
        // NSWorkspace + NSAppleEventManager.
        "macos" => println!("cargo:rustc-link-lib=framework=AppKit"),
        _ => {}
    }
}
