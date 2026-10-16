# Waterkit System

System information and status monitoring.

## Features

- **Connectivity**: Active connection type (Wi-Fi, cellular, Ethernet, ...).
- **Thermal**: Thermal state (nominal, fair, serious, critical), where the
  device reports one.
- **Load**: CPU usage, where the platform exposes it, and memory usage.

Every query returns `SystemError` when the platform bridge fails; a value the
device does not offer is `None`.

## Installation

```toml
[dependencies]
waterkit-system = "0.1"
```

## Platform Support

| Platform | Backend |
| :--- | :--- |
| **macOS/iOS** | `ProcessInfo`, `NWPathMonitor`, Mach host statistics |
| **Android** | `ConnectivityManager`, `PowerManager`, `ActivityManager` |
| **Linux** | NetworkManager over D-Bus, or the kernel's interface and route tables where NetworkManager does not run; `sysinfo` |
| **Windows** | Network List Manager, IP Helper adapter table; `sysinfo` |

On Android, `connectivity()` needs `android.permission.ACCESS_NETWORK_STATE`
in the application manifest, and CPU usage is `None`: applications cannot read
system-wide CPU statistics since Android 8.

## Usage

```rust
use waterkit_system::{SystemError, connectivity, thermal_state};

fn check_system() -> Result<(), SystemError> {
    let network = connectivity()?;
    println!("Network: {:?}", network.connection_type()); // e.g., Wifi

    match thermal_state()? {
        Some(thermal) => println!("Thermal: {thermal:?}"), // e.g., Nominal
        None => println!("Thermal state not reported by this device"),
    }
    Ok(())
}
```
