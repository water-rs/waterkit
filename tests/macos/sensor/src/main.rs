//! macOS test for waterkit-sensor
//!
//! Run with: cargo run -p waterkit-sensor-test

use waterkit_sensor::{Accelerometer, AmbientLight, Barometer, Gyroscope, Magnetometer};

#[tokio::main]
async fn main() {
    println!("=== Sensor Availability Test ===\n");

    // Check availability of each sensor
    println!(
        "Accelerometer available: {}",
        Accelerometer::capabilities().available
    );
    println!(
        "Gyroscope available:     {}",
        Gyroscope::capabilities().available
    );
    println!(
        "Magnetometer available:  {}",
        Magnetometer::capabilities().available
    );
    println!(
        "Barometer available:     {}",
        Barometer::capabilities().available
    );
    println!(
        "Ambient Light available: {}",
        AmbientLight::capabilities().available
    );

    println!("\n=== Attempting Sensor Reads ===\n");

    // Try to read accelerometer
    match Accelerometer::read().await {
        Ok(data) => println!(
            "Accelerometer: x={:.3}, y={:.3}, z={:.3}",
            data.x(),
            data.y(),
            data.z()
        ),
        Err(e) => println!("Accelerometer: {}", e),
    }

    // Try to read gyroscope
    match Gyroscope::read().await {
        Ok(data) => println!(
            "Gyroscope:     x={:.3}, y={:.3}, z={:.3}",
            data.x(),
            data.y(),
            data.z()
        ),
        Err(e) => println!("Gyroscope:     {}", e),
    }

    // Try to read magnetometer
    match Magnetometer::read().await {
        Ok(data) => println!(
            "Magnetometer:  x={:.3}, y={:.3}, z={:.3}",
            data.x(),
            data.y(),
            data.z()
        ),
        Err(e) => println!("Magnetometer:  {}", e),
    }

    // Try to read barometer
    match Barometer::read().await {
        Ok(data) => println!("Barometer:     {:.2} hPa", data.value()),
        Err(e) => println!("Barometer:     {}", e),
    }

    // Try to read ambient light
    match AmbientLight::read().await {
        Ok(data) => println!("Ambient Light: {:.2} (approximate)", data.value()),
        Err(e) => println!("Ambient Light: {}", e),
    }

    println!("\n=== Test Complete ===");
}
