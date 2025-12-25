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
            match waterkit_content::authenticate("Test Auth").await {
                Ok(_) => println!("Biometric Auth SUCCESS"),
                Err(e) => println!("Biometric Auth FAILED: {:?}", e),
            }
        }

        #[cfg(feature = "location")]
        {
            println!("Testing waterkit-location...");
            match waterkit_content::LocationManager::get_location_unchecked().await {
                Ok(loc) => println!("Location: lat={}, lon={}", loc.latitude, loc.longitude),
                Err(e) => println!("Location FAILED: {:?}", e),
            }
        }
    });

    println!("=== Test Complete ===");
}
