#[swift_bridge::bridge]
mod ffi {
    extern "Rust" {
        fn run_tests();
    }
}

fn run_tests() {
    println!("=== Generic iOS Test Runner ===");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async {
        #[cfg(feature = "sensor")]
        {
            println!("Testing waterkit-sensor...");
            if waterkit::sensor::Accelerometer::is_available() {
                match waterkit::sensor::Accelerometer::read().await {
                    Ok(data) => println!("Accelerometer: x={} y={} z={}", data.x, data.y, data.z),
                    Err(e) => println!("Accelerometer read failed: {e:?}"),
                }
            }
        }

        #[cfg(feature = "biometric")]
        {
            println!("Testing waterkit-biometric...");
            match waterkit::biometric::authenticate("Test Auth").await {
                Ok(_) => println!("Biometric Auth SUCCESS"),
                Err(e) => println!("Biometric Auth FAILED: {e:?}"),
            }
        }

        #[cfg(feature = "location")]
        {
            println!("Testing waterkit-location...");
            match waterkit::location::Location::get().await {
                Ok(loc) => println!("Location: lat={}, lon={}", loc.latitude(), loc.longitude()),
                Err(e) => println!("Location FAILED: {e:?}"),
            }
        }

        #[cfg(feature = "audio")]
        {
            println!("Testing waterkit-audio...");
            println!("Audio: API available");
        }

        #[cfg(feature = "camera")]
        {
            println!("Testing waterkit-camera...");
            match waterkit::camera::Camera::list() {
                Ok(cams) => println!("Found {} cameras", cams.len()),
                Err(e) => println!("Camera list failed: {e:?}"),
            }
            println!("Camera: API available (display requires View)");
        }

        #[cfg(feature = "clipboard")]
        {
            println!("Testing waterkit-clipboard...");
            match waterkit::clipboard::Clipboard::new() {
                Ok(mut clipboard) => match clipboard.set_text("WaterKit Test") {
                    Ok(()) => println!("Clipboard: set_text SUCCESS"),
                    Err(e) => println!("Clipboard: set_text FAILED: {e:?}"),
                },
                Err(e) => println!("Clipboard init FAILED: {e:?}"),
            }
        }

        #[cfg(feature = "codec")]
        {
            println!("Testing waterkit-codec...");
            println!("Codec: API available");
        }

        #[cfg(feature = "dialog")]
        {
            println!("Testing waterkit-dialog...");
            println!("Dialog: API available");
        }

        #[cfg(feature = "fs")]
        {
            println!("Testing waterkit-fs...");
            if let Some(path) = waterkit::fs::WaterFs::cache_dir() {
                println!("FS cache_dir: {path:?}");
            }
        }

        #[cfg(feature = "haptic")]
        {
            println!("Testing waterkit-haptic...");
            match waterkit::haptic::Haptic::notification_success() {
                Ok(_) => println!("Haptic: feedback SUCCESS"),
                Err(e) => println!("Haptic FAILED: {e:?}"),
            }
        }

        #[cfg(feature = "notification")]
        {
            println!("Testing waterkit-notification...");
            let result = waterkit::notification::Notification::new()
                .title("WaterKit Test")
                .body("iOS notification is working!")
                .show();
            println!("Notification result: {result:?}");
        }

        #[cfg(feature = "permission")]
        {
            println!("Testing waterkit-permission...");
            let status =
                waterkit::permission::check(waterkit::permission::Permission::Location).await;
            println!("Permission status: {status:?}");
        }

        #[cfg(feature = "secret")]
        {
            println!("Testing waterkit-secret...");
            match waterkit::secret::SecretManager::set("waterkit", "ios_test", "secret123").await {
                Ok(_) => println!("Secret set SUCCESS"),
                Err(e) => println!("Secret set FAILED: {e}"),
            }
            match waterkit::secret::SecretManager::get("waterkit", "ios_test").await {
                Ok(value) => println!("Secret get SUCCESS: {value}"),
                Err(e) => println!("Secret get FAILED: {e}"),
            }
            match waterkit::secret::SecretManager::delete("waterkit", "ios_test").await {
                Ok(_) => println!("Secret delete SUCCESS"),
                Err(e) => println!("Secret delete FAILED: {e}"),
            }
        }

        #[cfg(feature = "system")]
        {
            println!("Testing waterkit-system...");
            let conn = waterkit::system::get_connectivity_info();
            println!("Connectivity: {:?}", conn.connection_type);
        }

        #[cfg(feature = "video")]
        {
            println!("Testing waterkit-video...");
            println!("Video: API available (display requires View)");
        }

        #[cfg(feature = "screen")]
        {
            println!("Testing waterkit-screen...");
            match waterkit::screen::screens() {
                Ok(screens) => println!("Screen count: {}", screens.len()),
                Err(e) => println!("Screen failed: {e:?}"),
            }
        }

        #[cfg(feature = "bluetooth")]
        {
            println!("Testing waterkit-bluetooth...");
            match waterkit::bluetooth::adapter_state().await {
                Ok(state) => println!("Bluetooth state: {state:?}"),
                Err(e) => println!("Bluetooth FAILED: {e:?}"),
            }
        }

        #[cfg(feature = "nfc")]
        {
            println!("Testing waterkit-nfc...");
            println!("NFC available: {}", waterkit::nfc::is_available());
        }

        #[cfg(feature = "share")]
        {
            println!("Testing waterkit-share...");
            match waterkit::share::ShareSheet::text("WaterKit iOS share test")
                .show()
                .await
            {
                Ok(result) => println!("Share result: {result:?}"),
                Err(e) => println!("Share FAILED: {e:?}"),
            }
        }

        #[cfg(feature = "speech")]
        {
            println!("Testing waterkit-speech...");
            match waterkit::speech::Tts::new().await {
                Ok(tts) => {
                    let config = waterkit::speech::TtsConfig::default();
                    if let Err(e) = tts.speak("WaterKit iOS speech test", &config).await {
                        println!("Speech FAILED: {e:?}");
                    }
                    tts.stop();
                }
                Err(e) => println!("Speech init FAILED: {e:?}"),
            }
        }

        #[cfg(feature = "contacts")]
        {
            println!("Testing waterkit-contacts...");
            match waterkit::contacts::fetch_all().await {
                Ok(contacts) => println!("Contacts fetched: {}", contacts.len()),
                Err(e) => println!("Contacts FAILED: {e:?}"),
            }
        }

        #[cfg(feature = "calendar")]
        {
            println!("Testing waterkit-calendar...");
            match waterkit::calendar::list_calendars().await {
                Ok(calendars) => println!("Calendars fetched: {}", calendars.len()),
                Err(e) => println!("Calendar FAILED: {e:?}"),
            }
        }

        #[cfg(feature = "health")]
        {
            println!("Testing waterkit-health...");
            println!("Health available: {}", waterkit::health::is_available());
        }

        #[cfg(feature = "background")]
        {
            println!("Testing waterkit-background...");
            let capabilities = waterkit::background::capabilities();
            println!(
                "Background capabilities: refresh={} processing={} continued={} launch_events={}",
                capabilities.supports_app_refresh,
                capabilities.supports_processing,
                capabilities.supports_continued_processing,
                capabilities.supports_launch_events
            );
        }

        #[cfg(feature = "deeplink")]
        {
            println!("Testing waterkit-deeplink...");
            match waterkit::deeplink::can_open_url("https://example.com").await {
                Ok(can_open) => println!("can_open_url: {can_open}"),
                Err(e) => println!("can_open_url FAILED: {e:?}"),
            }
        }
    });

    println!("=== Test Complete ===");
}
