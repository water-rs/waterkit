//! Screen capture demo.
//!
//! Demonstrates the basic screen API: listing screens, brightness control, and screenshots.
//!
//! Run: `cargo run --example demo`

use waterkit_screen::{ImageFormat, get_brightness, screens, screenshot_primary, set_brightness};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("WaterKit Screen Demo\n");

    // 1. List screens
    let screen_list = screens()?;
    println!("Found {} screen(s):", screen_list.len());
    for screen in &screen_list {
        println!(
            "  - {}: {}x{} (scale: {:.1}x){}",
            screen.name,
            screen.width,
            screen.height,
            screen.scale_factor,
            if screen.is_primary { " [primary]" } else { "" }
        );
    }

    // 2. Brightness control
    println!("\nBrightness:");
    match get_brightness().await {
        Ok(b) => {
            println!("  Current: {:.0}%", b * 100.0);
            // Set brightness to itself (safe test)
            if let Err(e) = set_brightness(b).await {
                println!("  Set failed: {e}");
            }
        }
        Err(e) => println!("  Not available: {e}"),
    }

    // 3. Screenshot
    println!("\nCapturing screenshot...");
    match screenshot_primary(ImageFormat::Png) {
        Ok(shot) => {
            let filename = "screenshot.png";
            shot.save(filename)?;
            println!(
                "  Saved: {} ({}x{}, {} bytes)",
                filename,
                shot.width,
                shot.height,
                shot.data.len()
            );
        }
        Err(e) => println!("  Failed: {e}"),
    }

    println!("\nDone!");
    Ok(())
}
