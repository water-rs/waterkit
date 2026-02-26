//! System information and status.
//!
//! This crate provides a unified API for retrieving system information
//! such as connectivity, thermal state, and system load across different platforms.

mod sys;

/// Type of network connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConnectionType {
    /// WiFi connection.
    Wifi,
    /// Cellular data connection.
    Cellular,
    /// Ethernet connection.
    Ethernet,
    /// Bluetooth connection.
    Bluetooth,
    /// VPN connection.
    Vpn,
    /// Other connection type.
    Other,
    /// No connection (offline).
    None,
}

/// Information about network connectivity.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ConnectivityInfo {
    connection_type: ConnectionType,
    is_connected: bool,
}

impl ConnectivityInfo {
    /// Create a new `ConnectivityInfo`.
    #[must_use]
    pub(crate) const fn new(connection_type: ConnectionType, is_connected: bool) -> Self {
        Self {
            connection_type,
            is_connected,
        }
    }

    /// The type of the current connection.
    #[must_use]
    pub const fn connection_type(&self) -> ConnectionType {
        self.connection_type
    }

    /// Whether the device is connected to the internet.
    #[must_use]
    pub const fn is_connected(&self) -> bool {
        self.is_connected
    }
}

/// Thermal state of the device.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ThermalState {
    /// Normal operating temperature.
    Nominal,
    /// Slightly elevated temperature.
    Fair,
    /// High temperature, performance may be throttled.
    Serious,
    /// Critical temperature, performance is significantly throttled.
    Critical,
    /// Thermal state is unknown.
    Unknown,
}

/// Information about system load.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SystemLoad {
    cpu_usage: f32,
    memory_used: u64,
    memory_total: u64,
}

impl SystemLoad {
    /// Create a new `SystemLoad`.
    #[must_use]
    pub(crate) const fn new(cpu_usage: f32, memory_used: u64, memory_total: u64) -> Self {
        Self {
            cpu_usage,
            memory_used,
            memory_total,
        }
    }

    /// CPU usage percentage (0.0 - 100.0).
    #[must_use]
    pub const fn cpu_usage(&self) -> f32 {
        self.cpu_usage
    }

    /// Amount of used memory in bytes.
    #[must_use]
    pub const fn memory_used(&self) -> u64 {
        self.memory_used
    }

    /// Total amount of memory in bytes.
    #[must_use]
    pub const fn memory_total(&self) -> u64 {
        self.memory_total
    }
}

/// Get the current network connectivity information.
#[must_use]
pub fn get_connectivity_info() -> ConnectivityInfo {
    sys::get_connectivity_info()
}

/// Get the current thermal state of the device.
#[must_use]
pub fn get_thermal_state() -> ThermalState {
    sys::get_thermal_state()
}

/// Get the current system load information.
#[must_use]
pub fn get_system_load() -> SystemLoad {
    sys::get_system_load()
}
