//! Build script for waterkit-health.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" || target_os == "macos" {
        waterkit_build::build_apple_bridge(["src/sys/apple/mod.rs"]);
    }

    if target_os == "android" {
        let manifest_dir = std::path::PathBuf::from(
            std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR missing"),
        );
        let third_party = manifest_dir.join("third_party").join("health_connect");
        let jars = collect_jars(&third_party);

        let config = waterkit_build::AndroidConfig {
            extra_classpath: jars,
        };
        waterkit_build::build_kotlin_with_config(&["src/sys/android/HealthHelper.kt"], &config);
    }
}

fn collect_jars(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut jars = Vec::new();
    collect_jars_into(root, &mut jars);
    jars.sort();
    jars
}

fn collect_jars_into(path: &std::path::Path, jars: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(path).expect("health jar directory must be readable") {
        let entry = entry.expect("health jar directory entry must be readable");
        let path = entry.path();
        if path.is_dir() {
            collect_jars_into(&path, jars);
        } else if path.extension().is_some_and(|extension| extension == "jar") {
            jars.push(path);
        }
    }
}
