//! Android JNI generic test harness.

#![cfg(target_os = "android")]
#![allow(non_snake_case)]
#![allow(clippy::cargo_common_metadata)]

use jni::JNIEnv;
use jni::objects::{JClass, JObject};

// This harness expects `waterkit_content` to be available.
// The CLI ensures this dependency is injected.
// We use a feature flag or conditional compilation to avoid checking errors when the dep is missing during normal builds?
// No, the harness crate is *only* useful when driven by CLI.
// But to allow `cargo check` on the workspace, we might need a dummy fallback.
// Or we just accept that `waterkit-test-android` won't compile without modification.

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_waterkit_test_MainActivity_runTest(
    mut _env: JNIEnv,
    _class: JClass,
    _activity: JObject,
) {
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );

    // Initialize sensor system with context
    // The activity is a Context, so we can pass it directly.
    if let Err(e) = waterkit_content::sys::android::init(&mut _env, &_activity) {
        log::error!("Failed to initialize sensor subsystem: {}", e);
        return;
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async {
        log::info!("=== Generic Android Test Runner ===");
        log::info!("Checking sensor availability...");

        if waterkit_content::Accelerometer::is_available() {
            log::info!("Accelerometer: Available");
            match waterkit_content::Accelerometer::read().await {
                Ok(data) => log::info!(
                    "Accelerometer Read: x={:.2} y={:.2} z={:.2}",
                    data.x,
                    data.y,
                    data.z
                ),
                Err(e) => log::error!("Accelerometer Read Error: {}", e),
            }
        } else {
            log::warn!("Accelerometer: Not Available");
        }

        if waterkit_content::Gyroscope::is_available() {
            log::info!("Gyroscope: Available");
            match waterkit_content::Gyroscope::read().await {
                Ok(data) => log::info!(
                    "Gyroscope Read: x={:.2} y={:.2} z={:.2}",
                    data.x,
                    data.y,
                    data.z
                ),
                Err(e) => log::error!("Gyroscope Read Error: {}", e),
            }
        } else {
            log::warn!("Gyroscope: Not Available");
        }

        if waterkit_content::Magnetometer::is_available() {
            log::info!("Magnetometer: Available");
            match waterkit_content::Magnetometer::read().await {
                Ok(data) => log::info!(
                    "Magnetometer Read: x={:.2} y={:.2} z={:.2}",
                    data.x,
                    data.y,
                    data.z
                ),
                Err(e) => log::error!("Magnetometer Read Error: {}", e),
            }
        } else {
            log::warn!("Magnetometer: Not Available");
        }

        if waterkit_content::Barometer::is_available() {
            log::info!("Barometer: Available");
            match waterkit_content::Barometer::read().await {
                Ok(data) => log::info!("Barometer Read: {:.2}", data.value),
                Err(e) => log::error!("Barometer Read Error: {}", e),
            }
        } else {
            log::warn!("Barometer: Not Available");
        }

        if waterkit_content::AmbientLight::is_available() {
            log::info!("AmbientLight: Available");
            match waterkit_content::AmbientLight::read().await {
                Ok(data) => log::info!("AmbientLight Read: {:.2}", data.value),
                Err(e) => log::error!("AmbientLight Read Error: {}", e),
            }
        } else {
            log::warn!("AmbientLight: Not Available");
        }

        log::info!("=== Test Complete ===");
    });
}
