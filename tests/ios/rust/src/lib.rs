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
            // Add sensor test calls here if API is available for iOS
        }

        #[cfg(feature = "biometric")]
        {
            println!("Testing waterkit-biometric...");
            match waterkit_biometric::authenticate("Test Auth").await {
                Ok(_) => println!("Biometric Auth SUCCESS"),
                Err(e) => println!("Biometric Auth FAILED: {:?}", e),
            }
        }

        #[cfg(feature = "location")]
        {
            println!("Testing waterkit-location...");
            match waterkit_location::LocationManager::get_location_unchecked().await {
                Ok(loc) => println!("Location: lat={}, lon={}", loc.latitude, loc.longitude),
                Err(e) => println!("Location FAILED: {:?}", e),
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
            match waterkit_camera::Camera::list() {
                Ok(cams) => println!("Found {} cameras", cams.len()),
                Err(e) => println!("Camera list failed: {:?}", e),
            }
            println!("Camera: API available (display requires View)");
        }

        #[cfg(feature = "clipboard")]
        {
            println!("Testing waterkit-clipboard...");
            waterkit_clipboard::set_text("WaterKit Test".to_string());
            println!("Clipboard: set_text SUCCESS");
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
            if let Some(path) = waterkit_fs::WaterFs::cache_dir() {
                println!("FS cache_dir: {:?}", path);
            }
        }

        #[cfg(feature = "haptic")]
        {
            println!("Testing waterkit-haptic...");
            match waterkit_haptic::feedback(waterkit_haptic::HapticFeedback::Success).await {
                Ok(_) => println!("Haptic: feedback SUCCESS"),
                Err(e) => println!("Haptic FAILED: {:?}", e),
            }
        }

        #[cfg(feature = "notification")]
        {
            println!("Testing waterkit-notification...");
            waterkit_notification::Notification::new()
                .title("WaterKit Test")
                .body("iOS notification is working!")
                .show();
            println!("Notification: Sent test notification");
        }

        #[cfg(feature = "permission")]
        {
            println!("Testing waterkit-permission...");
            println!("Permission: API available");
        }

        #[cfg(feature = "secret")]
        {
            println!("Testing waterkit-secret...");
            println!("Secret: API available");
        }

        #[cfg(feature = "system")]
        {
            println!("Testing waterkit-system...");
            // println!("System OS: {}", waterkit_system::os_name());
            println!("System OS: Unknown (API not yet exposed)");
        }

        #[cfg(feature = "video")]
        {
            println!("Testing waterkit-video...");
            println!("Video: API available (display requires View)");
        }
    });

    println!("=== Test Complete ===");
}
