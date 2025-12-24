use std::thread;
use std::time::Duration;
use waterkit_system::{get_connectivity_info, get_system_load, get_thermal_state};

fn main() {
    println!("Checking system info...");

    // Connectivity
    let connectivity = get_connectivity_info();
    println!("Connectivity: {:?}", connectivity);

    // Thermal
    let thermal = get_thermal_state();
    println!("Thermal State: {:?}", thermal);

    // Load
    println!("Measuring system load (waiting 1s)...");
    let load = get_system_load();
    println!("System Load: {:?}", load);
    println!("CPU: {:.1}%", load.cpu_usage);
    println!("Mem Used: {} / {}", load.memory_used, load.memory_total);
}
