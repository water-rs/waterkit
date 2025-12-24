# Waterkit Location

Geolocation services for cross-platform apps.

## Features

- **Get Location**: One-shot current location query.
- **Tracking**: (Roadmap) Continuous location updates.
- **Accuracy**: Configurable accuracy requirements.

## Installation

```toml
[dependencies]
waterkit-location = "0.1"
```

## Platform Support

| Platform | Backend |
| :--- | :--- |
| **macOS/iOS** | `CoreLocation` |
| **Android** | `LocationManager` |
| **Windows** | `Windows.Devices.Geolocation` |
| **Linux** | *Geoclue (Planned)* |

## Usage

```rust
use waterkit_location::LocationManager;

async fn where_am_i() {
    let manager = LocationManager::new().await.unwrap();
    
    match manager.get_current_location().await {
        Ok(loc) => println!("Lat: {}, Long: {}", loc.latitude, loc.longitude),
        Err(e) => eprintln!("Error: {}", e),
    }
}
```

## Permissions

**iOS**: Add `NSLocationWhenInUseUsageDescription`.
**Android**: Add `<uses-permission android:name="android.permission.ACCESS_FINE_LOCATION" />`.
