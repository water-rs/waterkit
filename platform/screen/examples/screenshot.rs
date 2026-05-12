//! Simple screenshot example.
//!
//! Demonstrates capturing a screenshot in various formats (PNG, HEIF, AVIF).
//!
//! Run: `cargo run --example screenshot`

use waterkit_screen::{ImageFormat, screens, screenshot_primary};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("WaterKit Screen - Screenshot Example");

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

    println!("\nCapturing screenshot as PNG...");
    let shot = screenshot_primary(ImageFormat::Png)?;
    let filename = format!("screenshot_{}x{}.png", shot.width(), shot.height());
    shot.save(&filename)?;
    println!("  Saved: {} ({} bytes)", filename, shot.data().len());

    println!("\nCapturing screenshot as HEIF...");
    match screenshot_primary(ImageFormat::Heif) {
        Ok(shot) => {
            let filename = format!("screenshot_{}x{}.heic", shot.width(), shot.height());
            shot.save(&filename)?;
            println!("  Saved: {} ({} bytes)", filename, shot.data().len());
        }
        Err(e) => println!("  HEIF not supported: {e}"),
    }

    println!("\nCapturing screenshot as AVIF...");
    match screenshot_primary(ImageFormat::Avif) {
        Ok(shot) => {
            let filename = format!("screenshot_{}x{}.avif", shot.width(), shot.height());
            shot.save(&filename)?;
            println!("  Saved: {} ({} bytes)", filename, shot.data().len());
        }
        Err(e) => println!("  AVIF not supported: {e}"),
    }

    println!("\nDone!");
    Ok(())
}
