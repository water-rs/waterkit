mod sys;

pub use sys::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionType {
    Wifi,
    Cellular,
    Ethernet,
    Bluetooth,
    Vpn,
    Other,
    None, // Offline
}

#[derive(Debug, Clone)]
pub struct ConnectivityInfo {
    pub connection_type: ConnectionType,
    pub is_connected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThermalState {
    Nominal,
    Fair,
    Serious,
    Critical,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct SystemLoad {
    pub cpu_usage: f32, // 0.0 - 100.0%
    pub memory_used: u64,
    pub memory_total: u64,
}

pub fn get_connectivity_info() -> ConnectivityInfo {
    sys::get_connectivity_info()
}

pub fn get_thermal_state() -> ThermalState {
    sys::get_thermal_state()
}

pub fn get_system_load() -> SystemLoad {
    sys::get_system_load()
}
