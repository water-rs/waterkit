//! System info demo.
use waterkit_system::{connectivity, load, thermal_state};

fn main() {
    println!("Checking system info...");

    let net = connectivity();
    println!("Connectivity: {net:?}");

    let thermal = thermal_state();
    println!("Thermal State: {thermal:?}");

    println!("Measuring system load (waiting 1s)...");
    let load = load();
    println!("System Load: {load:?}");
    println!("CPU: {:.1}%", load.cpu_usage());
    println!("Mem Used: {} / {}", load.memory_used(), load.memory_total());
}
