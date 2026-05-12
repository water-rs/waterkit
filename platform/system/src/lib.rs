//! System information and status.
//!
//! Snapshot APIs ([`connectivity`], [`thermal_state`], [`load`]) return
//! the current value at the moment of the call. A future revision will
//! pair each with a `Subscribed<T>`-returning variant once platform
//! change-listeners are wired in.

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

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

/// Snapshot of the current network connectivity.
#[must_use]
pub fn connectivity() -> ConnectivityInfo {
    sys::get_connectivity_info()
}

/// Snapshot of the current thermal state.
#[must_use]
pub fn thermal_state() -> ThermalState {
    sys::get_thermal_state()
}

/// Snapshot of the current CPU / memory load.
#[must_use]
pub fn load() -> SystemLoad {
    sys::get_system_load()
}
