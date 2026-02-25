# waterkit-health

Cross-platform health data access for Rust.

Part of the [Waterkit](https://github.com/water-rs/waterkit) ecosystem.

## Features

- Read and write health/fitness data samples
- Authorization management for health data types
- Supports: steps, heart rate, active energy, distance, weight, height, blood oxygen, sleep

## Platform Support

| Platform | Status |
|----------|--------|
| iOS      | Native (HealthKit via Swift bridge) |
| macOS    | Native (HealthKit via Swift bridge) |
| Android  | Availability check only (Health Connect ops pending) |
| Windows  | Not supported |
| Linux    | Not supported |

## Usage

```rust
use waterkit_health::{
    is_available, request_authorization, query_samples,
    HealthDataType, HealthSample,
};

async fn example() -> Result<(), waterkit_health::HealthError> {
    // Check availability
    if !is_available() {
        return Err(waterkit_health::HealthError::NotAvailable);
    }

    // Request authorization
    request_authorization(
        &[HealthDataType::Steps, HealthDataType::HeartRate],
        &[],
    ).await?;

    // Query step count samples
    let samples = query_samples(
        HealthDataType::Steps,
        "2025-01-01T00:00:00Z",
        "2025-01-31T23:59:59Z",
    ).await?;

    Ok(())
}
```

## License

MIT OR Apache-2.0
