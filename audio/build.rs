//! Build script for waterkit-audio.
//!
//! Handles platform-specific code generation:
//! - Apple: Swift bridge generation + Swift compilation
//! - Android: Kotlin → DEX compilation

use std::{env, path::PathBuf, process::Command};

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();

    // Apple platforms: generate Swift bridge and compile Swift
    if target_os == "ios" || target_os == "macos" {
        build_apple();
    }

    // Android: compile Kotlin to DEX
    if target_os == "android" {
        build_android();
    }
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
#[allow(clippy::too_many_lines)]
fn build_apple() {
    use std::fs;

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    // Swift source and generated bridge header file
    let swift_source = manifest_dir.join("src/sys/apple/MediaHelper.swift");
    let audio_player_swift = manifest_dir.join("src/sys/apple/AudioPlayerHelper.swift");
    let bridge_rs = "src/sys/apple/mod.rs";

    println!("cargo:rerun-if-changed={bridge_rs}");
    println!("cargo:rerun-if-changed={}", swift_source.display());
    println!("cargo:rerun-if-changed={}", audio_player_swift.display());

    // 1. Generate Swift bridge code
    let bridges = vec![bridge_rs];
    swift_bridge_build::parse_bridges(bridges)
        .write_all_concatenated(out_dir.clone(), env!("CARGO_PKG_NAME"));

    // 2. Create combined bridging header
    let core_h = out_dir.join("SwiftBridgeCore.h");
    let pkg_h = out_dir.join("waterkit-audio/waterkit-audio.h");
    let bridging_h = out_dir.join("Bridging-Header.h");

    let bridging_content = format!(
        "#include \"{}\"\n#include \"{}\"\n",
        core_h.display(),
        pkg_h.display()
    );
    fs::write(&bridging_h, bridging_content).expect("Failed to write bridging header");

    // 3. Concatenate all Swift sources into one file
    let core_swift = out_dir.join("SwiftBridgeCore.swift");
    let gen_swift = out_dir.join("waterkit-audio/waterkit-audio.swift");
    let combined_swift = out_dir.join("CombinedMedia.swift");

    let core_content =
        fs::read_to_string(&core_swift).expect("Failed to read SwiftBridgeCore.swift");
    let gen_content = fs::read_to_string(&gen_swift).expect("Failed to read generated swift");
    let impl_content = fs::read_to_string(&swift_source).expect("Failed to read MediaHelper.swift");
    let audio_player_content =
        fs::read_to_string(&audio_player_swift).expect("Failed to read AudioPlayerHelper.swift");

    fs::write(
        &combined_swift,
        format!("{core_content}\n{gen_content}\n{impl_content}\n{audio_player_content}"),
    )
    .expect("Failed to write combined Swift file");

    // 4. Compile Swift to object file
    let obj_file = out_dir.join("MediaHelper.o");

    let target = env::var("TARGET").unwrap();
    let sdk = if target.contains("ios") {
        "iphoneos"
    } else {
        "macosx"
    };

    let sdk_path = String::from_utf8(
        Command::new("xcrun")
            .args(["--sdk", sdk, "--show-sdk-path"])
            .output()
            .expect("xcrun failed")
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let mut swiftc = Command::new("swiftc");
    swiftc
        .arg("-emit-object")
        .arg("-o")
        .arg(&obj_file)
        .arg("-sdk")
        .arg(&sdk_path)
        .arg("-import-objc-header")
        .arg(&bridging_h)
        .arg("-parse-as-library")
        .arg("-module-name")
        .arg("MediaHelper")
        .arg(&combined_swift);

    // Add target triple for cross-compilation
    if target.contains("ios") {
        swiftc.arg("-target").arg("arm64-apple-ios14.0");
    } else if target.contains("aarch64") {
        swiftc.arg("-target").arg("arm64-apple-macos11.0");
    } else {
        swiftc.arg("-target").arg("x86_64-apple-macos11.0");
    }

    let output = swiftc.output().expect("Failed to run swiftc");
    if !output.status.success() {
        eprintln!(
            "Swift compilation command: swiftc args: {:?}",
            swiftc.get_args().collect::<Vec<_>>()
        );
        eprintln!("Swift compilation failed:");
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        panic!("Swift compilation failed");
    }

    // Create static library from object file
    let lib_file = out_dir.join("libMediaHelper.a");
    let ar_status = Command::new("ar")
        .args([
            "rcs",
            lib_file.to_str().unwrap(),
            obj_file.to_str().unwrap(),
        ])
        .status()
        .expect("Failed to run ar");
    assert!(ar_status.success(), "ar failed");

    // Link the Swift library
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=MediaHelper");

    // Link Swift runtime
    let toolchain_dir = String::from_utf8(
        Command::new("xcrun")
            .args(["--find", "swiftc"])
            .output()
            .expect("xcrun --find swiftc failed")
            .stdout,
    )
    .unwrap();
    let toolchain_lib = PathBuf::from(toolchain_dir.trim())
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("lib/swift/macosx");
    println!("cargo:rustc-link-search=native={}", toolchain_lib.display());

    // Link required frameworks
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=MediaPlayer");
    println!("cargo:rustc-link-lib=framework=AVFoundation");
    if target.contains("ios") {
        println!("cargo:rustc-link-lib=framework=UIKit");
    } else {
        println!("cargo:rustc-link-lib=framework=AppKit");
    }
}

#[cfg(not(any(target_os = "ios", target_os = "macos")))]
fn build_apple() {}

fn build_android() {
    const KOTLIN_FILE_RELATIVE_PATH: &str = "src/sys/android/MediaSessionHelper.kt";

    println!("cargo:rerun-if-changed={KOTLIN_FILE_RELATIVE_PATH}");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let kotlin_file =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join(KOTLIN_FILE_RELATIVE_PATH);

    let android_jar_path = android_build::android_jar(None).expect("Failed to find android.jar");

    // Compile .kt -> .class using kotlinc
    let classes_dir = out_dir.join("classes");
    std::fs::create_dir_all(&classes_dir).expect("Failed to create classes directory");

    let kotlinc_output = Command::new("kotlinc")
        .arg("-classpath")
        .arg(&android_jar_path)
        .arg("-d")
        .arg(&classes_dir)
        .arg(&kotlin_file)
        .output()
        .expect("Failed to run kotlinc - is Kotlin compiler installed?");

    if !kotlinc_output.status.success() {
        eprintln!(
            "kotlinc stderr: {}",
            String::from_utf8_lossy(&kotlinc_output.stderr)
        );
        panic!("kotlinc compilation failed");
    }

    // Find all .class files recursively
    let mut class_files = Vec::new();
    find_class_files(&classes_dir, &mut class_files);

    assert!(
        !class_files.is_empty(),
        "No .class files generated by kotlinc"
    );

    let d8_jar_path = android_build::android_d8_jar(None).expect("Failed to find d8.jar");

    // Convert .class -> .dex using D8
    let mut java_run = android_build::JavaRun::new();
    java_run
        .class_path(&d8_jar_path)
        .main_class("com.android.tools.r8.D8")
        .arg("--classpath")
        .arg(&android_jar_path)
        .arg("--output")
        .arg(&out_dir);

    for class_file in &class_files {
        java_run.arg(class_file);
    }

    let d8_result = java_run
        .run()
        .expect("failed to acquire exit status for java d8.jar invocation");

    assert!(d8_result.success(), "D8 dexing failed");
}

fn find_class_files(dir: &PathBuf, results: &mut Vec<PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                find_class_files(&path, results);
            } else if path.extension().is_some_and(|e| e == "class") {
                results.push(path);
            }
        }
    }
}
