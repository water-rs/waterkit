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

| Platform | Backend | `LocationProvider` |
| :--- | :--- | :--- |
| **macOS/iOS** | `CoreLocation` | `CoreLocation` |
| **Android** with Google Play services | Fused Location Provider (`play-services-location`) | `FusedLocationProvider` |
| **Android** without Google Play services | `LocationManager` (`FUSED_PROVIDER` from API 31 when registered, else GPS / network) | `AndroidLocationManager` |
| **Windows** | `Windows.Devices.Geolocation` | `WindowsGeolocation` |
| **Linux** | `GeoClue2` (when installed on the system bus) | `GeoClue` |
| **Browser** | Geolocation API (when exposed) | `BrowserGeolocation` |

`Location::capabilities().await` reports the provider that serves the device;
`provider` is `None` (and `available()` false) where none does.

On Android the realization is chosen once per process, before the first
request, by asking `GoogleApiAvailability` whether Google Play services is
usable. A failed request returns its error and is never retried through the
other realization. The crate declares
`com.google.android.gms:play-services-location` under
`[package.metadata.waterui.android] maven`, so the packager links the thin
client; the provider itself is the Google Play services installed on the
device.

## Usage

```rust
use waterkit_location::{Location, LocationError};

async fn provider() -> Option<waterkit_location::LocationProvider> {
    Location::capabilities().await.provider
}

async fn where_am_i() -> Result<(f64, f64), LocationError> {
    let loc = Location::get().await?;
    Ok((loc.latitude().get(), loc.longitude().get()))
}
```

## Permissions

**iOS**: Add `NSLocationWhenInUseUsageDescription`.
**Android**: Add `<uses-permission android:name="android.permission.ACCESS_FINE_LOCATION" />`.
