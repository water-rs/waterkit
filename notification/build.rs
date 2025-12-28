//! Build script for waterkit-notification.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os == "ios" || target_os == "macos" {
        use waterkit_build::AppleSwiftConfig;

        let config = AppleSwiftConfig::new("waterkit-notification", "NotificationHelper")
            .swift_source("src/sys/apple/Notification.swift")
            .framework("Foundation")
            .framework("UserNotifications");

        waterkit_build::compile_swift("src/sys/apple/mod.rs", &config);
    }

    if target_os == "android" {
        waterkit_build::build_kotlin(&["src/sys/android/NotificationHelper.kt"]);
    }
}
