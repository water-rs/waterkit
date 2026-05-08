//! Android JNI generic test harness.

#![cfg(target_os = "android")]

use jni::JNIEnv;
use jni::objects::JObject;
use jni::sys::jdoubleArray;

const PERMISSION_NOT_DETERMINED: i32 = 0;
#[cfg(feature = "permission")]
const PERMISSION_RESTRICTED: i32 = 1;
#[cfg(feature = "permission")]
const PERMISSION_DENIED: i32 = 2;
#[cfg(feature = "permission")]
const PERMISSION_GRANTED: i32 = 3;

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_waterkit_test_MainActivity_runTest(
    env: JNIEnv,
    _this: JObject,
    activity: JObject,
) {
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );

    let activity_global = match env.new_global_ref(&activity) {
        Ok(value) => value,
        Err(e) => {
            log::error!("Failed to create global ref for activity: {e}");
            return;
        }
    };
    let java_vm = match env.get_java_vm() {
        Ok(vm) => vm,
        Err(e) => {
            log::error!("Failed to get Java VM: {e}");
            return;
        }
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime for Android test harness");

    rt.block_on(async {
        log::info!("=== Generic Android Test Runner ===");
        let mut env = match java_vm.get_env() {
            Ok(value) => value,
            Err(e) => {
                log::error!("Failed to attach/get JNIEnv: {e}");
                return;
            }
        };
        let activity = activity_global.as_obj();

        #[cfg(feature = "sensor")]
        {
            log::info!("Testing waterkit-sensor...");
            if waterkit_content::sensor::Accelerometer::capabilities().available {
                match waterkit_content::sensor::Accelerometer::read().await {
                    Ok(data) => log::info!(
                        "Accelerometer Read: x={:.2} y={:.2} z={:.2}",
                        data.x,
                        data.y,
                        data.z
                    ),
                    Err(e) => log::error!("Accelerometer Read Error: {e}"),
                }
            } else {
                log::warn!("Accelerometer not available");
            }
        }

        #[cfg(feature = "biometric")]
        {
            log::info!("Testing waterkit-biometric...");
            let caps = waterkit_content::biometric::capabilities().await;
            log::info!("Biometric available: {} kind: {:?}", caps.available, caps.kind);
            match waterkit_content::biometric::android::authenticate_with_context(
                &mut env,
                activity,
                "Waterkit Android harness",
            ) {
                Ok(rx) => match rx.await {
                    Ok(Ok(())) => log::info!("Biometric Auth SUCCESS"),
                    Ok(Err(e)) => log::error!("Biometric Auth FAILED: {e}"),
                    Err(e) => log::error!("Biometric Auth channel FAILED: {e}"),
                },
                Err(e) => log::error!("Biometric Auth init FAILED: {e}"),
            }
        }

        #[cfg(feature = "location")]
        {
            log::info!("Testing waterkit-location...");
            match waterkit_content::location::android::get_location_with_context(&mut env, activity)
            {
                Ok(loc) => {
                    log::info!("Location: lat={}, lon={}", loc.latitude(), loc.longitude());
                }
                Err(e) => log::error!("Location FAILED: {e}"),
            }
        }

        #[cfg(feature = "audio")]
        {
            log::info!("Testing waterkit-audio...");
            log::info!("Audio API linked");
        }

        #[cfg(feature = "camera")]
        {
            log::info!("Testing waterkit-camera...");
            use waterkit_content::camera::Camera;
            match Camera::list() {
                Ok(cameras) => {
                    log::info!("Camera List: Found {} cameras", cameras.len());
                    for cam in &cameras {
                        log::info!("  - ID: {}, Name: {}", cam.id, cam.name);
                    }
                }
                Err(e) => log::error!("Camera List FAILED: {e}"),
            }
        }

        #[cfg(feature = "clipboard")]
        {
            log::info!("Testing waterkit-clipboard...");
            match waterkit_content::clipboard::Clipboard::new() {
                Ok(mut clipboard) => {
                    match clipboard.set_text("WaterKit Test") {
                        Ok(()) => log::info!("Clipboard set_text SUCCESS"),
                        Err(e) => log::error!("Clipboard set_text FAILED: {e}"),
                    }
                    match clipboard.text().await {
                        Ok(text) => log::info!("Clipboard get_text = {text:?}"),
                        Err(e) => log::error!("Clipboard get_text FAILED: {e}"),
                    }
                }
                Err(e) => log::error!("Clipboard init FAILED: {e}"),
            }
        }

        #[cfg(feature = "codec")]
        {
            log::info!("Testing waterkit-codec...");
            log::info!("Codec API linked");
        }

        #[cfg(feature = "dialog")]
        {
            log::info!("Testing waterkit-dialog...");
            log::info!("Dialog API linked");
        }

        #[cfg(feature = "fs")]
        {
            log::info!("Testing waterkit-fs...");
            match waterkit_content::fs::WaterFs::cache_dir_with_context(&mut env, activity) {
                Ok(path) => log::info!("FS cache_dir: {path:?}"),
                Err(error) => log::error!("FS cache_dir unavailable: {error}"),
            }
        }

        #[cfg(feature = "haptic")]
        {
            log::info!("Testing waterkit-haptic...");
            match waterkit_content::haptic::Haptic::impact(waterkit_content::haptic::Intensity::LOW)
            {
                Ok(()) => log::info!("Haptic feedback SUCCESS"),
                Err(e) => log::error!("Haptic feedback FAILED: {e}"),
            }
        }

        #[cfg(feature = "notification")]
        {
            log::info!("Testing waterkit-notification...");
            let result = waterkit_content::notification::Notification::new()
                .title("WaterKit Android Harness")
                .body("notification test")
                .show();
            log::info!("Notification show result: {result:?}");
        }

        #[cfg(feature = "permission")]
        {
            log::info!("Testing waterkit-permission...");
            match waterkit_content::permission::android::check_with_activity(
                &mut env,
                activity,
                waterkit_content::permission::Permission::Location,
            ) {
                Ok(status) => log::info!("Permission status: {status:?}"),
                Err(e) => log::error!("Permission check FAILED: {e}"),
            }
        }

        #[cfg(feature = "secret")]
        {
            log::info!("Testing waterkit-secret...");
            match waterkit_content::secret::android::set_with_context(
                &mut env,
                activity,
                "waterkit",
                "test",
                "secret123",
            ) {
                Ok(_) => log::info!("Secret set SUCCESS"),
                Err(e) => log::error!("Secret set FAILED: {e}"),
            }
            match waterkit_content::secret::android::get_with_context(
                &mut env, activity, "waterkit", "test",
            ) {
                Ok(val) => log::info!("Secret get = {val:?}"),
                Err(e) => log::error!("Secret get FAILED: {e}"),
            }
            match waterkit_content::secret::android::delete_with_context(
                &mut env, activity, "waterkit", "test",
            ) {
                Ok(_) => log::info!("Secret delete SUCCESS"),
                Err(e) => log::error!("Secret delete FAILED: {e}"),
            }
        }

        #[cfg(feature = "system")]
        {
            log::info!("Testing waterkit-system...");
            let conn = waterkit_content::system::connectivity();
            log::info!("System connectivity: {:?}", conn.connection_type);
            let thermal = waterkit_content::system::thermal_state();
            log::info!("System thermal: {thermal:?}");
        }

        #[cfg(feature = "video")]
        {
            log::info!("Testing waterkit-video...");
            log::info!("Video API linked");
        }

        #[cfg(feature = "bluetooth")]
        {
            log::info!("Testing waterkit-bluetooth...");
            match waterkit_content::bluetooth::android::get_adapter_state(&mut env, activity) {
                Ok(state) => log::info!("Bluetooth adapter state: {state:?}"),
                Err(e) => log::error!("Bluetooth adapter state FAILED: {e}"),
            }
        }

        #[cfg(feature = "nfc")]
        {
            log::info!("Testing waterkit-nfc...");
            match waterkit_content::nfc::android::is_available(&mut env, activity) {
                Ok(available) => log::info!("NFC available: {available}"),
                Err(e) => log::error!("NFC availability FAILED: {e}"),
            }
        }

        #[cfg(feature = "share")]
        {
            log::info!("Testing waterkit-share...");
            let sheet = waterkit_content::share::ShareSheet::text("WaterKit share test");
            match waterkit_content::share::android::share_with_context(&mut env, activity, &sheet) {
                Ok(result) => log::info!("Share result: {result:?}"),
                Err(e) => log::error!("Share FAILED: {e}"),
            }
        }

        #[cfg(feature = "speech")]
        {
            log::info!("Testing waterkit-speech...");
            let recognizer_available = waterkit_content::speech::SpeechRecognizer::capabilities();
            log::info!("Speech recognizer available: {recognizer_available}");
            if let Err(e) = waterkit_content::speech::android::init_with_context(&mut env, activity)
            {
                log::error!("Speech context init FAILED: {e}");
            } else {
                match waterkit_content::speech::Tts::new().await {
                    Ok(tts) => {
                        log::info!("TTS created, currently speaking: {}", tts.is_speaking());
                        let config = waterkit_content::speech::TtsConfig::default();
                        if let Err(e) = tts.speak("WaterKit speech test", &config).await {
                            log::error!("TTS speak FAILED: {e}");
                        }
                        tts.stop();
                    }
                    Err(e) => log::error!("TTS init FAILED: {e}"),
                }
            }
        }

        #[cfg(feature = "contacts")]
        {
            log::info!("Testing waterkit-contacts...");
            match waterkit_content::contacts::fetch_all().await {
                Ok(contacts) => log::info!("Contacts fetched: {}", contacts.len()),
                Err(e) => log::error!("Contacts fetch FAILED: {e}"),
            }
        }

        #[cfg(feature = "calendar")]
        {
            log::info!("Testing waterkit-calendar...");
            match waterkit_content::calendar::list_calendars().await {
                Ok(calendars) => log::info!("Calendars fetched: {}", calendars.len()),
                Err(e) => log::error!("Calendar list FAILED: {e}"),
            }
        }

        #[cfg(feature = "health")]
        {
            log::info!("Testing waterkit-health...");
            let available = waterkit_content::health::is_available();
            log::info!("Health available: {available}");
        }

        #[cfg(feature = "background")]
        {
            log::info!("Testing waterkit-background...");
            let capabilities = waterkit_content::background::capabilities();
            log::info!(
                "Background capabilities: refresh={} processing={} continued={} launch_events={}",
                capabilities.supports_app_refresh,
                capabilities.supports_processing,
                capabilities.supports_continued_processing,
                capabilities.supports_launch_events
            );
        }

        #[cfg(feature = "passkey")]
        {
            log::info!("Testing waterkit-passkey...");
            match waterkit_content::passkey::is_available().await {
                Ok(availability) => log::info!(
                    "Passkey availability: supported={} uv={} discoverable={}",
                    availability.is_platform_supported,
                    availability.supports_user_verification,
                    availability.supports_discoverable_credentials
                ),
                Err(e) => log::error!("Passkey availability FAILED: {e}"),
            }
        }

        #[cfg(feature = "deeplink")]
        {
            log::info!("Testing waterkit-deeplink...");
            match waterkit_content::deeplink::android::can_open_url_with_context(
                &mut env,
                activity,
                "https://example.com",
            ) {
                Ok(can_open) => log::info!("can_open_url(https://example.com): {can_open}"),
                Err(e) => log::error!("can_open_url FAILED: {e}"),
            }
            if let Err(e) = waterkit_content::deeplink::android::open_url_with_context(
                &mut env,
                activity,
                "https://example.com",
            ) {
                log::error!("open_url FAILED: {e}");
            }
        }

        #[cfg(feature = "screen")]
        {
            log::info!("Testing waterkit-screen...");
            match waterkit_content::screen::screens() {
                Ok(screens) => log::info!("Screen count: {}", screens.len()),
                Err(e) => log::error!("Screen enumeration FAILED: {e}"),
            }
        }

        log::info!("=== Test Complete ===");
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_waterkit_test_MainActivity_testCheckPermission(
    mut env: JNIEnv,
    _this: JObject,
    activity: JObject,
    _permission_type: i32,
) -> i32 {
    #[cfg(feature = "permission")]
    {
        let permission = match _permission_type {
            0 => waterkit_content::permission::Permission::Location,
            1 => waterkit_content::permission::Permission::Camera,
            2 => waterkit_content::permission::Permission::Microphone,
            3 => waterkit_content::permission::Permission::Photos,
            4 => waterkit_content::permission::Permission::Contacts,
            5 => waterkit_content::permission::Permission::Calendar,
            _ => {
                log::error!("Unknown permission type: {_permission_type}");
                return PERMISSION_NOT_DETERMINED;
            }
        };

        return match waterkit_content::permission::android::check_with_activity(
            &mut env, &activity, permission,
        ) {
            Ok(waterkit_content::permission::PermissionStatus::NotDetermined) => {
                PERMISSION_NOT_DETERMINED
            }
            Ok(waterkit_content::permission::PermissionStatus::Restricted) => PERMISSION_RESTRICTED,
            Ok(waterkit_content::permission::PermissionStatus::Denied) => PERMISSION_DENIED,
            Ok(waterkit_content::permission::PermissionStatus::Granted) => PERMISSION_GRANTED,
            Err(e) => {
                log::error!("Permission check failed: {e}");
                PERMISSION_NOT_DETERMINED
            }
        };
    }

    #[cfg(not(feature = "permission"))]
    {
        let _ = (&mut env, &activity);
        log::error!("testCheckPermission called without enabling permission feature");
        PERMISSION_NOT_DETERMINED
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_waterkit_test_MainActivity_testGetLocation(
    env: JNIEnv,
    _this: JObject,
    activity: JObject,
) -> jdoubleArray {
    #[cfg(feature = "location")]
    {
        let mut env = env;
        match waterkit_content::location::android::get_location_with_context(&mut env, &activity) {
            Ok(location) => {
                let altitude = location.altitude().unwrap_or(0.0);
                let accuracy = location.horizontal_accuracy().unwrap_or(0.0);
                let payload = [
                    1.0,
                    location.latitude(),
                    location.longitude(),
                    altitude,
                    accuracy,
                ];

                let array = match env.new_double_array(payload.len() as i32) {
                    Ok(arr) => arr,
                    Err(e) => {
                        log::error!("new_double_array failed: {e}");
                        return std::ptr::null_mut();
                    }
                };

                if let Err(e) = env.set_double_array_region(&array, 0, &payload) {
                    log::error!("set_double_array_region failed: {e}");
                    return std::ptr::null_mut();
                }

                return array.into_raw();
            }
            Err(e) => {
                log::error!("Location test failed: {e}");
                return std::ptr::null_mut();
            }
        }
    }

    #[cfg(not(feature = "location"))]
    {
        let _ = (&env, &activity);
        log::error!("testGetLocation called without enabling location feature");
        std::ptr::null_mut()
    }
}
