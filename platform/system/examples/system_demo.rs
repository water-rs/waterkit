//! System info demo.
use waterkit_system::{SystemError, connectivity, load, thermal_state};

fn main() -> Result<(), SystemError> {
    println!("Checking system info...");

    let net = futures::executor::block_on(connectivity())?;
    println!("Connectivity: {net:?}");

    match thermal_state()? {
        Some(thermal) => println!("Thermal State: {thermal:?}"),
        None => println!("Thermal State: not reported by this device"),
    }

    println!("Measuring system load...");
    let load = load()?;
    println!("System Load: {load:?}");
    match load.cpu_usage() {
        Some(cpu) => println!("CPU: {cpu:.1}%"),
        None => println!("CPU: not exposed by this platform"),
    }
    println!("Mem Used: {} / {}", load.memory_used(), load.memory_total());
    Ok(())
}
