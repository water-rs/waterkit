# Waterkit Sensor

Device sensor access (Accelerometer, Gyroscope, etc.).

## Features

- **Sensors**: Accelerometer, Gyroscope, Magnetometer, Barometer, Ambient Light.
- **Reactive**: Stream-based updates.

## Installation

```toml
[dependencies]
waterkit-sensor = "0.1"
```

## Platform Support

| Platform | Backend |
| :--- | :--- |
| **macOS/iOS** | `CoreMotion` |
| **Android** | `SensorManager` |
| **Desktop** | *Hardware dependent (often unavailable)* |

## Usage

```rust
use waterkit_sensor::{Accelerometer, SensorData};

async fn read_motion() {
    if Accelerometer::is_available().await {
        let subscription = Accelerometer::listen().await;
        
        while let Some(data) = subscription.next().await {
             println!("X: {}, Y: {}, Z: {}", data.x, data.y, data.z);
        }
    }
}
```
