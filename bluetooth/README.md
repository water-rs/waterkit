# waterkit-bluetooth

Cross-platform Bluetooth (BLE and Classic) for Rust.

Part of the [Waterkit](https://github.com/water-rs/waterkit) ecosystem.

## Features

- **BLE**: Scanning, connecting, GATT service/characteristic read/write/notify
- **Classic Bluetooth**: Device discovery, pairing, serial port (SPP) connections
- Async-first API with `async-channel` for streaming results

## Platform Support

| Platform | Status |
|----------|--------|
| iOS      | Native (CoreBluetooth via Swift bridge) |
| macOS    | Native (CoreBluetooth via Swift bridge) |
| Android  | Native (BluetoothAdapter via JNI/Kotlin) |
| Windows  | Native (Windows.Devices.Bluetooth) |
| Linux    | D-Bus (BlueZ) |

Linux SPP note: RFCOMM channel discovery uses `sdptool` from BlueZ userspace tools.

## Usage

```rust
use waterkit_bluetooth::{BleScanner, ScanFilter, BleConnection};

async fn scan_and_connect() -> Result<(), waterkit_bluetooth::BluetoothError> {
    // Check adapter state
    let state = waterkit_bluetooth::adapter_state().await?;

    // Scan for BLE peripherals
    let scanner = BleScanner::new().await?;
    let rx = scanner.start_scan(&ScanFilter::default())?;

    // Receive scan results
    let result = rx.recv().await.unwrap();
    scanner.stop_scan();

    // Connect and discover services
    let conn = BleConnection::connect(&result.device.id).await?;
    let services = conn.discover_services().await?;

    Ok(())
}
```

## License

MIT OR Apache-2.0
