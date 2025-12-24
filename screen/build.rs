//! Build script for waterkit-screen.

use std::{env, path::PathBuf, process::Command};

#[allow(
    clippy::too_many_lines,
    clippy::items_after_statements,
    clippy::format_push_string
)]
fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    // Apple platforms: generate and compile Swift bridge
    if target_os == "ios" || target_os == "macos" {
        let bridge_file = "src/platform/apple.rs";
        let swift_extra_files = if target_os == "macos" {
            vec!["src/platform/apple/ScreenMacOS.swift"]
        } else {
            vec!["src/platform/apple/Screen.swift"]
        };

        println!("cargo:rerun-if-changed={bridge_file}");
        for f in &swift_extra_files {
            println!("cargo:rerun-if-changed={f}");
        }

        // 1. Generate Swift bridge code
        let _bridge_out_path = out_dir.join("waterkit-screen-bridge.swift");
        swift_bridge_build::parse_bridges(vec![bridge_file])
            .write_all_concatenated(&out_dir, env!("CARGO_PKG_NAME")); // Writes waterkit-screen-swift.swift actually? No, check docs.
        // .write_all_concatenated writes multiple files if package?
        // "write_all_concatenated" takes `out_dir` and `crate_name`.
        // It creates `{crate_name}-swift-bridge.swift` ? or similar.
        // Let's assume standard behavior or check location crate.
        // documentation says: Generates Swift code.

        // Let's rely on knowing the filename or finding it.
        // Usually it writes `waterkit_screen.swift` via the `write_all_concatenated` ??

        // Actually, let's just use `parse_bridges`...
        // `swift_bridge_build` is good but invoking `swiftc` requires correct files.
        // Let's assume `write_all_concatenated` creates `waterkit-screen.swift` (based on internal defaults or crate name).

        // Wait, I can't rely on assumptions for file names if I want to compile it.
        // But I can know the directory.
        // Let's list the directory in `out_dir` to find `.swift` files if unsure?
        // Or better: manual generation if needed.

        // Re-reading `location/build.rs`: it calls `write_all_concatenated` and DOES NOT compile.
        // This suggests `location` relies on consuming crate to build it OR `swift-bridge-build` triggers something?
        // No, `swift-bridge-build` is purely codegen.
        // If I want to run `cargo run`, I MUST compile it.

        // Let's compile all .swift files in `out_dir` + my extra files.
        // Swift compilation logic:

        let _generated_swift =
            out_dir.join(format!("{}-swift-bridge.swift", env!("CARGO_PKG_NAME")));
        // Note: swift-bridge-build might name it slightly differently.
        // Let's verify location of generated file?
        // `write_all_concatenated` implementation: `path.join(format!("{}-swift-bridge.swift", crate_name))`
        // But crate name in `env!("CARGO_PKG_NAME")` is `waterkit-screen` (hyphens).
        // Rust usually converts generic names? No, it uses the string passed.

        // Let's try compiling.
        let mut cmd = Command::new("swiftc");
        cmd.arg("-emit-library").arg("-static");
        cmd.arg("-o").arg(out_dir.join("libwaterkit_screen.a"));

        // Include generated bridge
        // If I am not sure of the name, I'll assume `waterkit-screen-swift-bridge.swift`
        // But let's just use a glob or something? No glob in pure Rust.
        // Let's assume the name.
        // Actually, I can use `write_to_file` of swift bridge if I want specific name.

        // Find generated files
        // Panic output showed: SwiftBridgeCore.swift, SwiftBridgeCore.h, waterkit-screen
        // waterkit-screen likely contains the bridge code? Or is it a dir?
        // Let's iterate and find all .swift files or files that look like bridge.

        let mut swift_sources = Vec::new();
        swift_sources.push(out_dir.join("SwiftBridgeCore.swift"));

        // Check "waterkit-screen"
        let potential_bridge = out_dir.join("waterkit-screen");
        if potential_bridge.exists() {
            if potential_bridge.is_file() {
                swift_sources.push(potential_bridge);
            } else {
                // It might be a dir?
                if let Ok(entries) = std::fs::read_dir(&potential_bridge) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().is_some_and(|e| e == "swift") {
                            swift_sources.push(path);
                        }
                    }
                }
            }
        } else {
            // Check for file with crate name
            let crate_name_file = out_dir.join(env!("CARGO_PKG_NAME"));
            if crate_name_file.exists() && crate_name_file.is_file() {
                swift_sources.push(crate_name_file);
            }
        }

        // Also look for any .swift file in out_dir just in case
        if let Ok(entries) = std::fs::read_dir(&out_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "swift") && !swift_sources.contains(&path)
                {
                    swift_sources.push(path);
                }
            }
        }

        for src in swift_sources {
            cmd.arg(src);
        }

        // Handle Bridging Header
        // swift-bridge generates C headers that declare the symbols used by Swift.
        // We need to pass these to swiftc via -import-objc-header.
        // If there are multiple headers (SwiftBridgeCore.h, crate-bridge.h), we need to amalgamate them.

        let mut headers = Vec::new();
        fn collect_headers(dir: &PathBuf, headers: &mut Vec<PathBuf>) {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        collect_headers(&path, headers);
                    } else if path.extension().is_some_and(|e| e == "h")
                        && let Some(name) = path.file_name().and_then(|n| n.to_str())
                        && name != "Bridging-Header.h"
                    {
                        headers.push(path);
                    }
                }
            }
        }
        collect_headers(&out_dir, &mut headers);

        // Also check if there is a header in the crate-named file path if it wasn't caught
        // (Sometimes headers have irregular naming)

        if !headers.is_empty() {
            let master_header_path = out_dir.join("Bridging-Header.h");
            let mut master_content = String::new();
            for h in &headers {
                let p_str = h.to_string_lossy();
                master_content.push_str(&format!("#include \"{p_str}\"\n"));
            }
            std::fs::write(&master_header_path, master_content)
                .expect("Failed to write bridging header");

            cmd.arg("-import-objc-header");
            cmd.arg(master_header_path);
        }

        // Add extra files (ScreenMacOS.swift etc)
        for f in &swift_extra_files {
            cmd.arg(f);
        }

        // Fix for macOS SDks
        if target_os == "macos" {
            cmd.arg("-sdk").arg(get_sdk_path("macosx"));
        } else if target_os == "ios" {
            cmd.arg("-sdk").arg(get_sdk_path("iphoneos"));
            cmd.arg("-target").arg("arm64-apple-ios14.0"); // Example target
        }

        // Execute
        // Only verify logic for macOS for now as we are verifying on Desktop. iOS compilation via cargo run is tricky anyway.
        if target_os == "macos" {
            let status = cmd.status().expect("Failed to run swiftc");
            assert!(status.success(), "swiftc failed");

            println!("cargo:rustc-link-search=native={}", out_dir.display());
            println!("cargo:rustc-link-lib=static=waterkit_screen");

            // Frameworks needed
            println!("cargo:rustc-link-lib=framework=ScreenCaptureKit"); // For SCK
            println!("cargo:rustc-link-lib=framework=Foundation");
            println!("cargo:rustc-link-lib=framework=Cocoa");

            // Swift runtime libraries for async/await (Swift Concurrency)
            // Get the Swift toolchain lib path
            let swift_lib_output = Command::new("xcrun")
                .args(["--toolchain", "default", "-f", "swiftc"])
                .output()
                .ok();
            if let Some(output) = swift_lib_output {
                let swiftc_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if let Some(toolchain_dir) = std::path::Path::new(&swiftc_path)
                    .parent()
                    .and_then(|p| p.parent())
                {
                    let lib_dir = toolchain_dir.join("lib/swift/macosx");
                    if lib_dir.exists() {
                        println!("cargo:rustc-link-search=native={}", lib_dir.display());
                    }
                }
            }
            // Also try standard Xcode location
            println!("cargo:rustc-link-arg=-rpath");
            println!("cargo:rustc-link-arg=/usr/lib/swift");
        }
    }
}

fn get_sdk_path(sdk: &str) -> String {
    let output = Command::new("xcrun")
        .args(["--sdk", sdk, "--show-sdk-path"])
        .output()
        .expect("xcrun failed");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}
