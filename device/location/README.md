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
| **Android** | Android location services |
| **Windows** | `Windows.Devices.Geolocation` |
| **Linux** | `GeoClue2` |

## Usage

```rust
use waterkit_location::{Location, LocationError};

async fn where_am_i() -> Result<(f64, f64), LocationError> {
    let loc = Location::get().await?;
    Ok((loc.latitude().get(), loc.longitude().get()))
}
```

## Permissions

**iOS**: Add `NSLocationWhenInUseUsageDescription`.
**Android**: Add `<uses-permission android:name="android.permission.ACCESS_FINE_LOCATION" />`.
