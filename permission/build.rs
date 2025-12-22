//! Build script for waterkit-permission.
//!
//! Handles platform-specific code generation:
//! - Apple: Swift bridge generation
//! - Android: Kotlin → DEX compilation

use std::{env, path::PathBuf, process::Command};

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();

    // Apple platforms: generate Swift bridge
    if target_os == "ios" || target_os == "macos" {
        build_apple();
    }

    // Android: compile Kotlin to DEX
    if target_os == "android" {
        build_android();
    }
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
fn build_apple() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    let bridges = vec!["src/sys/apple/mod.rs"];
    for bridge in &bridges {
        println!("cargo:rerun-if-changed={bridge}");
    }

    swift_bridge_build::parse_bridges(bridges)
        .write_all_concatenated(out_dir.clone(), env!("CARGO_PKG_NAME"));
}

#[cfg(not(any(target_os = "ios", target_os = "macos")))]
fn build_apple() {}

#[cfg(target_os = "android")]
fn build_android() {
    const KOTLIN_FILE_RELATIVE_PATH: &str = "src/sys/android/PermissionHelper.kt";

    println!("cargo:rerun-if-changed={KOTLIN_FILE_RELATIVE_PATH}");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let kotlin_file =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join(KOTLIN_FILE_RELATIVE_PATH);

    let android_jar_path = android_build::android_jar(None).expect("Failed to find android.jar");

    // Compile .kt -> .class using kotlinc
    let classes_dir = out_dir.join("classes");
    std::fs::create_dir_all(&classes_dir).expect("Failed to create classes directory");

    let kotlinc_status = Command::new("kotlinc")
        .arg("-classpath")
        .arg(&android_jar_path)
        .arg("-d")
        .arg(&classes_dir)
        .arg(&kotlin_file)
        .status()
        .expect("Failed to run kotlinc - is Kotlin compiler installed?");

    assert!(kotlinc_status.success(), "kotlinc compilation failed");

    let class_file = classes_dir
        .join("waterkit")
        .join("permission")
        .join("PermissionHelper.class");

    let d8_jar_path = android_build::android_d8_jar(None).expect("Failed to find d8.jar");

    // Convert .class -> .dex using D8
    assert!(
        android_build::JavaRun::new()
            .class_path(d8_jar_path)
            .main_class("com.android.tools.r8.D8")
            .arg("--classpath")
            .arg(android_jar_path)
            .arg("--output")
            .arg(&out_dir)
            .arg(&class_file)
            .run()
            .expect("failed to acquire exit status for java d8.jar invocation")
            .success(),
        "D8 dexing failed"
    );
}

#[cfg(not(target_os = "android"))]
fn build_android() {}
