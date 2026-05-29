//! Cross-platform Bluetooth (BLE and Classic).
//!
//! This crate provides unified APIs for:
//! - **BLE**: Scanning, connecting, GATT service/characteristic read/write/notify
//! - **Classic Bluetooth**: Device discovery, pairing, serial port (SPP) connections

#![warn(missing_docs)]

mod sys;

use std::collections::HashMap;

/// Android-specific JNI helpers that require `JNIEnv` and `Context`.
#[cfg(target_os = "android")]
pub mod android {
    pub use crate::sys::jni_api::get_adapter_state;
}

// ============================================================================
// Common
// ============================================================================

/// Unique identifier for a Bluetooth device.
///
/// Format is platform-specific: macOS uses `CoreBluetooth` identifier
/// strings, Android uses MAC addresses, Windows uses BLE device IDs,
/// Linux uses `BlueZ` object paths. Treat the inner string as opaque.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceId(String);

impl DeviceId {
    /// Wraps a platform-specific identifier string.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the inner identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Bluetooth device information.
#[derive(Debug, Clone)]
pub struct BluetoothDevice {
    /// Device identifier.
    pub id: DeviceId,
    /// Device name (if available).
    pub name: Option<String>,
    /// Signal strength in dBm (BLE only).
    pub rssi: Option<i16>,
    /// Whether the device is currently connected.
    pub is_connected: bool,
}

/// Bluetooth adapter state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterState {
    /// Bluetooth is available and powered on.
    PoweredOn,
    /// Bluetooth is available but powered off.
    PoweredOff,
    /// Bluetooth is not available on this device.
    Unavailable,
    /// Bluetooth permission has not been granted.
    Unauthorized,
    /// Adapter state is being determined.
    Unknown,
}

/// Get the current Bluetooth adapter state.
///
/// # Errors
/// Returns error if the state cannot be determined.
pub async fn adapter_state() -> Result<AdapterState, BluetoothError> {
    sys::adapter_state().await
}

// ============================================================================
// BLE
// ============================================================================

/// UUID for BLE services and characteristics.
///
/// Stored as a string because BLE supports 16-bit short UUIDs (e.g.
/// `0x180A`) alongside full 128-bit UUIDs, and platforms accept both
/// forms in different contexts. Treat the inner string as opaque.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Uuid(String);

impl Uuid {
    /// Wraps a UUID string. Accepts 16-bit (`0x180A`), 128-bit
    /// (`00001800-0000-1000-8000-00805F9B34FB`), or platform-specific
    /// formats.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the UUID string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Uuid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A BLE GATT service.
#[derive(Debug, Clone)]
pub struct GattService {
    /// Service UUID.
    pub uuid: Uuid,
    /// Whether this is a primary service.
    pub is_primary: bool,
    /// Characteristics belonging to this service.
    pub characteristics: Vec<GattCharacteristic>,
}

/// Properties of a GATT characteristic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct CharacteristicProperties {
    /// Can be read.
    pub read: bool,
    /// Can be written (with response).
    pub write: bool,
    /// Can be written (without response).
    pub write_without_response: bool,
    /// Supports notifications.
    pub notify: bool,
    /// Supports indications.
    pub indicate: bool,
}

/// A BLE GATT characteristic.
#[derive(Debug, Clone)]
pub struct GattCharacteristic {
    /// Characteristic UUID.
    pub uuid: Uuid,
    /// Characteristic properties.
    pub properties: CharacteristicProperties,
}

/// BLE scan filter.
#[derive(Debug, Clone, Default)]
pub struct ScanFilter {
    /// Filter by service UUIDs.
    pub service_uuids: Vec<Uuid>,
    /// Filter by device name prefix.
    pub name_prefix: Option<String>,
}

/// BLE scan result.
#[derive(Debug, Clone)]
pub struct ScanResult {
    /// The discovered device.
    pub device: BluetoothDevice,
    /// Advertised service UUIDs.
    pub service_uuids: Vec<Uuid>,
    /// Manufacturer-specific data keyed by company ID.
    pub manufacturer_data: HashMap<u16, Vec<u8>>,
}

/// BLE scanner for discovering peripherals.
#[derive(Debug)]
pub struct BleScanner {
    inner: sys::BleScannerInner,
}

impl BleScanner {
    /// Create a new BLE scanner.
    ///
    /// # Errors
    /// Returns error if Bluetooth is not available.
    pub async fn new() -> Result<Self, BluetoothError> {
        Ok(Self {
            inner: sys::BleScannerInner::new().await?,
        })
    }

    /// Start scanning for BLE peripherals.
    ///
    /// # Errors
    /// Returns error if scanning cannot be started.
    pub fn start_scan(
        &self,
        filter: &ScanFilter,
    ) -> Result<impl futures_core::Stream<Item = ScanResult> + Send + 'static, BluetoothError> {
        self.inner.start_scan(filter)
    }

    /// Stop scanning.
    pub fn stop_scan(&self) {
        self.inner.stop_scan();
    }
}

/// A connection to a BLE peripheral.
#[derive(Debug)]
pub struct BleConnection {
    inner: sys::BleConnectionInner,
}

impl BleConnection {
    /// Connect to a BLE peripheral by device ID.
    ///
    /// # Errors
    /// Returns error if connection fails.
    pub async fn connect(device_id: &DeviceId) -> Result<Self, BluetoothError> {
        Ok(Self {
            inner: sys::BleConnectionInner::connect(device_id).await?,
        })
    }

    /// Discover GATT services on the connected peripheral.
    ///
    /// # Errors
    /// Returns error if service discovery fails.
    #[allow(clippy::future_not_send)]
    pub async fn discover_services(&self) -> Result<Vec<GattService>, BluetoothError> {
        self.inner.discover_services().await
    }

    /// Read a characteristic value.
    ///
    /// # Errors
    /// Returns error if read fails.
    pub async fn read_characteristic(
        &self,
        service: &Uuid,
        characteristic: &Uuid,
    ) -> Result<Vec<u8>, BluetoothError> {
        self.inner
            .read_characteristic(service, characteristic)
            .await
    }

    /// Write a value to a characteristic.
    ///
    /// # Errors
    /// Returns error if write fails.
    #[allow(clippy::future_not_send)]
    pub async fn write_characteristic(
        &self,
        service: &Uuid,
        characteristic: &Uuid,
        data: &[u8],
    ) -> Result<(), BluetoothError> {
        self.inner
            .write_characteristic(service, characteristic, data)
            .await
    }

    /// Subscribe to characteristic notifications.
    ///
    /// # Errors
    /// Returns error if subscription fails.
    #[allow(clippy::future_not_send)]
    pub async fn subscribe(
        &self,
        service: &Uuid,
        characteristic: &Uuid,
    ) -> Result<impl futures_core::Stream<Item = Vec<u8>> + Send + 'static, BluetoothError> {
        self.inner.subscribe(service, characteristic).await
    }

    /// Disconnect from the peripheral.
    pub async fn disconnect(self) {
        self.inner.disconnect().await;
    }
}

// ============================================================================
// Classic Bluetooth
// ============================================================================

/// A classic Bluetooth device found during discovery.
#[derive(Debug, Clone)]
pub struct ClassicDevice {
    /// Device information.
    pub device: BluetoothDevice,
    /// Major device class.
    pub device_class: u32,
    /// Whether the device is paired/bonded.
    pub is_paired: bool,
}

/// Classic Bluetooth manager for discovery and SPP connections.
#[derive(Debug)]
pub struct ClassicBluetooth {
    inner: sys::ClassicBluetoothInner,
}

impl ClassicBluetooth {
    /// Create a new classic Bluetooth manager.
    ///
    /// # Errors
    /// Returns error if Bluetooth is not available.
    pub async fn new() -> Result<Self, BluetoothError> {
        Ok(Self {
            inner: sys::ClassicBluetoothInner::new().await?,
        })
    }

    /// Start classic device discovery.
    ///
    /// # Errors
    /// Returns error if discovery cannot be started.
    pub async fn start_discovery(
        &self,
    ) -> Result<impl futures_core::Stream<Item = ClassicDevice> + Send + 'static, BluetoothError>
    {
        self.inner.start_discovery().await
    }

    /// Stop device discovery.
    pub async fn stop_discovery(&self) {
        self.inner.stop_discovery().await;
    }

    /// Get list of paired/bonded devices.
    ///
    /// # Errors
    /// Returns error if the list cannot be retrieved.
    pub async fn paired_devices(&self) -> Result<Vec<ClassicDevice>, BluetoothError> {
        self.inner.paired_devices().await
    }

    /// Open an SPP (Serial Port Profile) connection to a device.
    ///
    /// # Errors
    /// Returns error if connection fails.
    pub async fn connect_spp(
        &self,
        device_id: &DeviceId,
        uuid: &Uuid,
    ) -> Result<SppStream, BluetoothError> {
        Ok(SppStream {
            inner: self.inner.connect_spp(device_id, uuid).await?,
        })
    }
}

/// A serial port (SPP) connection to a classic Bluetooth device.
#[derive(Debug)]
pub struct SppStream {
    inner: sys::SppStreamInner,
}

impl SppStream {
    /// Read data from the SPP stream.
    ///
    /// # Errors
    /// Returns error if read fails.
    #[allow(clippy::future_not_send)]
    pub async fn read(&self, buf: &mut [u8]) -> Result<usize, BluetoothError> {
        self.inner.read(buf).await
    }

    /// Write data to the SPP stream.
    ///
    /// # Errors
    /// Returns error if write fails.
    #[allow(clippy::future_not_send)]
    pub async fn write(&self, data: &[u8]) -> Result<usize, BluetoothError> {
        self.inner.write(data).await
    }

    /// Close the SPP connection.
    pub async fn close(self) {
        self.inner.close().await;
    }
}

// ============================================================================
// Errors
// ============================================================================

/// Errors that can occur in Bluetooth operations.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum BluetoothError {
    /// Bluetooth is not available on this device.
    #[error("Bluetooth not available")]
    NotAvailable,
    /// Bluetooth is powered off.
    #[error("Bluetooth is powered off")]
    PoweredOff,
    /// Permission not granted.
    #[error("Bluetooth permission not granted")]
    PermissionDenied,
    /// Device not found.
    #[error("device not found: {0}")]
    DeviceNotFound(String),
    /// Connection failed.
    #[error("connection failed: {0}")]
    ConnectionFailed(String),
    /// GATT operation failed.
    #[error("GATT error: {0}")]
    GattError(String),
    /// Not supported on this platform.
    #[error("not supported on this platform")]
    Unsupported,
    /// Platform-specific error.
    #[error("platform error: {0}")]
    Platform(String),
}
