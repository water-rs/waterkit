//! Screen capture demo.
//!
//! Demonstrates the basic screen API: listing screens, brightness control, and screenshots.
//!
//! Run: `cargo run --example demo`

use waterkit_screen::{Brightness, ImageFormat, brightness, screens, screenshot_primary, set_brightness};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("WaterKit Screen Demo\n");

    let screen_list = screens()?;
    println!("Found {} screen(s):", screen_list.len());
    for screen in &screen_list {
        println!(
            "  - {}: {}x{} (scale: {:.1}x){}",
            screen.name(),
            screen.width(),
            screen.height(),
            screen.scale_factor(),
            if screen.is_primary() { " [primary]" } else { "" }
        );
    }

    println!("\nBrightness:");
    match brightness().await {
        Ok(b) => {
            println!("  Current: {:.0}%", b.get() * 100.0);
            if let Err(e) = set_brightness(b).await {
                println!("  Set failed: {e}");
            }
        }
        Err(e) => println!("  Not available: {e}"),
    }

    println!("\nCapturing screenshot...");
    match screenshot_primary(ImageFormat::Png) {
        Ok(shot) => {
            let filename = "screenshot.png";
            shot.save(filename)?;
            println!(
                "  Saved: {} ({}x{}, {} bytes)",
                filename,
                shot.width(),
                shot.height(),
                shot.data().len()
            );
        }
        Err(e) => println!("  Failed: {e}"),
    }

    let _ = Brightness::MIN;
    println!("\nDone!");
    Ok(())
}
