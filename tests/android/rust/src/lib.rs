//! Android JNI generic test harness.

#![cfg(target_os = "android")]
#![allow(non_snake_case)]
#![allow(clippy::cargo_common_metadata)]

use jni::JNIEnv;
use jni::objects::JObject;

// This harness expects `waterkit_content` to be available.
// The CLI ensures this dependency is injected.
// We use a feature flag or conditional compilation to avoid checking errors when the dep is missing during normal builds?
// No, the harness crate is *only* useful when driven by CLI.
// But to allow `cargo check` on the workspace, we might need a dummy fallback.
// Or we just accept that `waterkit-test-android` won't compile without modification.

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_waterkit_test_MainActivity_runTest(
    mut _env: JNIEnv,
    _this: JObject,
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

    let activity_global = _env.new_global_ref(_activity).unwrap();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async {
        log::info!("=== Generic Android Test Runner ===");
        let java_vm = _env.get_java_vm().unwrap();
        let mut env = java_vm.get_env().unwrap();
        let activity = activity_global.as_obj();

        #[cfg(feature = "sensor")]
        {
            log::info!("Testing waterkit-sensor...");
            if waterkit_content::Accelerometer::is_available() {
                log::info!("Accelerometer: Available");
                match waterkit_content::Accelerometer::read().await {
                    Ok(data) => log::info!(
                        "Accelerometer Read: x={:.2} y={:.2} z={:.2}",
                        data.x, data.y, data.z
                    ),
                    Err(e) => log::error!("Accelerometer Read Error: {}", e),
                }
            }
        }

        #[cfg(feature = "biometric")]
        {
            log::info!("Testing waterkit-biometric...");
            match waterkit_content::sys::android::authenticate_with_context(&mut env, activity, "Test Auth") {
                Ok(rx) => {
                    match rx.await {
                        Ok(Ok(_)) => log::info!("Biometric Auth SUCCESS"),
                        Ok(Err(e)) => log::error!("Biometric Auth FAILED: {}", e),
                        Err(e) => log::error!("Biometric Auth CHANNEL ERROR: {}", e),
                    }
                }
                Err(e) => log::error!("Biometric Init FAILED: {}", e),
            }
        }

        #[cfg(feature = "location")]
        {
            log::info!("Testing waterkit-location...");
            match waterkit_content::sys::android::get_location_with_context(&mut env, activity) {
                Ok(loc) => log::info!("Location: lat={}, lon={}", loc.latitude, loc.longitude),
                Err(e) => log::error!("Location FAILED: {}", e),
            }
        }

        log::info!("=== Test Complete ===");
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_waterkit_test_MainActivity_testCheckPermission(
    _env: JNIEnv,
    _this: JObject,
    _activity: JObject,
    _permission_type: i32,
) -> i32 {
    3 // Granted
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_waterkit_test_MainActivity_testGetLocation(
    _env: JNIEnv,
    _this: JObject,
    _activity: JObject,
) -> JObject<'static> {
    JObject::null()
}
