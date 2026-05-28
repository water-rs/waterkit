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
use futures::StreamExt;
use waterkit_sensor::Accelerometer;

async fn read_motion() -> Result<(), waterkit_sensor::SensorError> {
    if Accelerometer::capabilities().available {
        let mut subscription = Accelerometer::watch(100)?;

        while let Some(data) = subscription.next().await {
            tracing::debug!(x = data.x(), y = data.y(), z = data.z(), "accelerometer sample");
        }
    }

    Ok(())
}
```
