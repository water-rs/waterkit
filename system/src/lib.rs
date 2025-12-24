//! System information and status.
//!
//! This crate provides a unified API for retrieving system information
//! such as connectivity, thermal state, and system load across different platforms.

mod sys;

/// Type of network connection.
#[derive(Debug, Clone, PartialEq, Eq)]
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
pub struct ConnectivityInfo {
    /// The type of the current connection.
    pub connection_type: ConnectionType,
    /// Whether the device is connected to the internet.
    pub is_connected: bool,
}

/// Thermal state of the device.
#[derive(Debug, Clone, PartialEq, Eq)]
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
pub struct SystemLoad {
    /// CPU usage percentage (0.0 - 100.0).
    pub cpu_usage: f32,
    /// Amount of used memory in bytes.
    pub memory_used: u64,
    /// Total amount of memory in bytes.
    pub memory_total: u64,
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
