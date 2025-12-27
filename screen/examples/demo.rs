//! Screen capture demo.
use std::io::Write;
use waterkit_screen::{capture_screen, get_brightness, screens, set_brightness};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("WaterKit Screen Demo");

    // 1. List screens
    let screen_list = screens()?;
    println!("Found {} screens:", screen_list.len());
    for screen in &screen_list {
        println!(
            " - {}: {}x{} (scale: {})",
            screen.name, screen.width, screen.height, screen.scale_factor
        );
    }

    // 2. Get Brightness
    match get_brightness().await {
        Ok(b) => println!("Current brightness: {b:.2}"),
        Err(e) => println!("Failed to get brightness: {e}"),
    }

    // 3. Set Brightness (Ask user or just try setting to current)
    // Be careful not to black out screen.
    // Let's just set it to itself to test API.
    if let Ok(b) = get_brightness().await {
        println!("Setting brightness to {b:.2}...");
        if let Err(e) = set_brightness(b).await {
            println!("Failed to set brightness: {e}");
        } else {
            println!("Brightness set successfully.");
        }
    }

    // 4. Capture Screen or Pick (if requested, but demo just shows basic usage)
    // Uncomment to test picker:
    /*
    println!("Launching System Picker...");
    match waterkit_screen::pick_and_capture().await {
        Ok(bytes) => {
             println!("Picked and captured {} bytes.", bytes.len());
             std::fs::write("picked.png", bytes)?;
        },
        Err(e) => println!("Pick failed: {}", e),
    }
    */

    // Existing capture logic
    if !screen_list.is_empty() {
        println!("Capturing main screen (index 0)...");
        match capture_screen(0) {
            Ok(bytes) => {
                println!("Captured {} bytes.", bytes.len());
                let filename = "screenshot.png";
                let mut file = std::fs::File::create(filename)?;
                file.write_all(&bytes)?;
                println!("Saved to {filename}");
            }
            Err(e) => println!("Failed to capture screen: {e}"),
        }
    }

    Ok(())
}
