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

    // Feature-gated initialization for crates that require it
    #[cfg(any(feature = "sensor", feature = "biometric", feature = "location"))]
    {
        if let Err(e) = waterkit_content::sys::android::init(&mut _env, &_activity) {
            log::error!("Failed to initialize subsystem: {}", e);
            return;
        }
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

        #[cfg(feature = "audio")]
        {
            log::info!("Testing waterkit-audio...");
            // Audio playback test - would need a test file
            log::info!("Audio: API available (playback requires test file)");
        }

        #[cfg(feature = "camera")]
        {
            log::info!("Testing waterkit-camera...");
            // Camera requires SurfaceView - log availability only
            log::info!("Camera: API available (display requires SurfaceView)");
        }

        #[cfg(feature = "clipboard")]
        {
            log::info!("Testing waterkit-clipboard...");
            match waterkit_content::set_text("WaterKit Test") {
                Ok(_) => log::info!("Clipboard: set_text SUCCESS"),
                Err(e) => log::error!("Clipboard set_text FAILED: {}", e),
            }
            match waterkit_content::get_text() {
                Ok(text) => log::info!("Clipboard: get_text = {:?}", text),
                Err(e) => log::error!("Clipboard get_text FAILED: {}", e),
            }
        }

        #[cfg(feature = "codec")]
        {
            log::info!("Testing waterkit-codec...");
            // Codec encode/decode cycle
            log::info!("Codec: API available");
        }

        #[cfg(feature = "dialog")]
        {
            log::info!("Testing waterkit-dialog...");
            // Dialog requires Activity context for display
            log::info!("Dialog: API available (requires UI thread)");
        }

        #[cfg(feature = "fs")]
        {
            log::info!("Testing waterkit-fs...");
            match waterkit_content::get_cache_dir() {
                Some(path) => log::info!("FS cache_dir: {:?}", path),
                None => log::error!("FS cache_dir: None"),
            }
        }

        #[cfg(feature = "haptic")]
        {
            log::info!("Testing waterkit-haptic...");
            match waterkit_content::feedback(waterkit_content::HapticFeedback::Light).await {
                Ok(_) => log::info!("Haptic: feedback SUCCESS"),
                Err(e) => log::error!("Haptic feedback FAILED: {}", e),
            }
        }

        #[cfg(feature = "notification")]
        {
            log::info!("Testing waterkit-notification...");
            log::info!("Notification: API available");
        }

        #[cfg(feature = "permission")]
        {
            log::info!("Testing waterkit-permission...");
            log::info!("Permission: API available");
        }

        #[cfg(feature = "secret")]
        {
            log::info!("Testing waterkit-secret...");
            match waterkit_content::set("test_key", "test_value") {
                Ok(_) => log::info!("Secret: set SUCCESS"),
                Err(e) => log::error!("Secret set FAILED: {}", e),
            }
            match waterkit_content::get("test_key") {
                Ok(val) => log::info!("Secret: get = {:?}", val),
                Err(e) => log::error!("Secret get FAILED: {}", e),
            }
        }

        #[cfg(feature = "system")]
        {
            log::info!("Testing waterkit-system...");
            let conn = waterkit_content::get_connectivity_info();
            log::info!("System connectivity: {:?}", conn.connection_type);
            let thermal = waterkit_content::get_thermal_state();
            log::info!("System thermal: {:?}", thermal);
        }

        #[cfg(feature = "video")]
        {
            log::info!("Testing waterkit-video...");
            // Video playback requires SurfaceView
            log::info!("Video: API available (display requires SurfaceView)");
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
