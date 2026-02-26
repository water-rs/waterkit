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
        let third_party = manifest_dir.join("third_party").join("health-connect");
        let jars = [
            "connect-client-1.1.0.jar",
            "connect-client-proto-1.1.0.jar",
            "connect-client-external-protobuf-1.1.0.jar",
            "activity-1.2.0.jar",
            "core-ktx-1.12.0.jar",
            "annotation-1.8.1.jar",
            "kotlinx-coroutines-core-jvm-1.7.3.jar",
            "kotlinx-coroutines-android-1.7.3.jar",
            "kotlinx-coroutines-guava-1.7.3.jar",
            "guava-31.1-android.jar",
            "failureaccess-1.0.1.jar",
            "jsr305-3.0.2.jar",
            "checker-qual-3.12.0.jar",
            "error_prone_annotations-2.11.0.jar",
            "j2objc-annotations-1.3.jar",
            "jspecify-1.0.0.jar",
        ]
        .into_iter()
        .map(|name| third_party.join(name))
        .collect::<Vec<_>>();

        let config = waterkit_build::AndroidConfig {
            extra_classpath: jars,
        };
        waterkit_build::build_kotlin_with_config(&["src/sys/android/HealthHelper.kt"], &config);
    }
}
