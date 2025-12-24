//! Build script for waterkit-sensor-test.
//!
//! Compiles the Swift code from the sensor crate and links CoreMotion.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    if target_os != "macos" && target_os != "ios" {
        return;
    }

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let sensor_crate = manifest_dir.join("../../../sensor");
    let swift_file = sensor_crate.join("src/sys/apple/sensor.swift");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    println!("cargo:rerun-if-changed={}", swift_file.display());

    // Generate swift-bridge files directly in test's OUT_DIR
    let bridges = vec![sensor_crate.join("src/sys/apple/mod.rs")];
    swift_bridge_build::parse_bridges(bridges)
        .write_all_concatenated(out_dir.clone(), "waterkit-sensor");

    let bridge_header = out_dir.join("waterkit-sensor").join("waterkit-sensor.h");
    let bridge_swift = out_dir.join("waterkit-sensor").join("waterkit-sensor.swift");

    // Compile Swift to static library
    let lib_path = out_dir.join("libsensor_swift.a");
    
    let output = Command::new("swiftc")
        .args([
            "-emit-library",
            "-static",
            "-module-name", "SensorSwift",
            "-import-objc-header", bridge_header.to_str().unwrap(),
            "-o", lib_path.to_str().unwrap(),
            swift_file.to_str().unwrap(),
            bridge_swift.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run swiftc");

    if !output.status.success() {
        eprintln!("Swift compilation stderr: {}", String::from_utf8_lossy(&output.stderr));
        eprintln!("Swift compilation stdout: {}", String::from_utf8_lossy(&output.stdout));
        panic!("Swift compilation failed");
    }

    // Link the static library and frameworks
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=sensor_swift");
    println!("cargo:rustc-link-lib=framework=CoreMotion");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=IOKit");
}
