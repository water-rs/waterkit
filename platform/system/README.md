# Waterkit System

System information and status monitoring.

## Features

- **Connectivity**: Check WiFi / Cellular status.
- **Battery**: Charge level, Charging status.
- **Thermal**: Thermal state (nominal, fair, serious, critical).
- **Device Info**: Model name, OS version.

## Installation

```toml
[dependencies]
waterkit-system = "0.1"
```

## Platform Support

| Platform | Backend |
| :--- | :--- |
| **macOS/iOS** | `ProcessInfo`, `NWPathMonitor` |
| **Android** | `ConnectivityManager`, `Build` |
| **Desktop** | `sysinfo` |

## Usage

```rust
use waterkit_system::{get_connectivity_status, get_thermal_state};

async fn check_system() {
    let status = get_connectivity_status().await;
    println!("Network: {:?}", status); // e.g., Wifi
    
    let thermal = get_thermal_state().await;
    println!("Thermal: {:?}", thermal); // e.g., Nominal
}
```
