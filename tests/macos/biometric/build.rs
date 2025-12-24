use std::path::PathBuf;

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    if target_os == "macos" || target_os == "ios" {
        let _out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());

        // Linking the biometrics framework is handled by swift-bridge/rustc usually if we use the bridge,
        // but since we are dependent on the library which has the bridge, we might need to link frameworks.
        // swift-bridge usually handles this if configured correctly.
        // However, for LocalAuthentication, we might need to link it explicitly or
        // let the biometric crate handle it.

        // Usually, we don't need a build.rs here if we just consume the crate,
        // BUT we need to support swift-bridge linking if we were compiling swift code here.
        // Since the swift code is in the dependency crate, the dependency crate's build.rs runs.
        // But we might need to link the static lib produced by that crate?
        // No, cargo handles rust dependencies.

        // HOWEVER, currently `swift-bridge` often requires the final binary to also participate in linking?
        // Let's set up a minimal build.rs just in case or to ensure correct rebuilding.
        println!("cargo:rerun-if-changed=build.rs");
    }
}
