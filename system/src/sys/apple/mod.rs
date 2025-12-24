use crate::{ConnectivityInfo, ConnectionType, SystemLoad, ThermalState};

#[swift_bridge::bridge]
mod ffi {
    pub enum ConnectionType {
        Wifi,
        Cellular,
        Ethernet,
        Bluetooth,
        Vpn,
        Other,
        None,
    }

    pub enum ThermalState {
        Nominal,
        Fair,
        Serious,
        Critical,
        Unknown,
    }

    #[swift_bridge(swift_repr = "struct")]
    pub struct RustConnectivityInfo {
        pub connection_type: ConnectionType,
        pub is_connected: bool,
    }

    // RustThermalState no longer needed as we return enum directly

    #[swift_bridge(swift_repr = "struct")]
    pub struct RustSystemLoad {
        pub cpu_usage: f32,
        pub memory_used: u64,
        pub memory_total: u64,
    }

    extern "Swift" {
        fn get_apple_connectivity() -> RustConnectivityInfo;
        fn get_apple_thermal_state() -> ThermalState;
        fn get_apple_system_load() -> RustSystemLoad;
    }
}

// ... existing helpers ...

pub fn get_connectivity_info() -> ConnectivityInfo {
    let info = ffi::get_apple_connectivity();
    let ct = match info.connection_type {
        ffi::ConnectionType::Wifi => ConnectionType::Wifi,
        ffi::ConnectionType::Cellular => ConnectionType::Cellular,
        ffi::ConnectionType::Ethernet => ConnectionType::Ethernet,
        ffi::ConnectionType::Bluetooth => ConnectionType::Bluetooth,
        ffi::ConnectionType::Vpn => ConnectionType::Vpn,
        ffi::ConnectionType::Other => ConnectionType::Other,
        ffi::ConnectionType::None => ConnectionType::None,
    };
    ConnectivityInfo {
        connection_type: ct,
        is_connected: info.is_connected,
    }
}

pub fn get_thermal_state() -> ThermalState {
    match ffi::get_apple_thermal_state() {
        ffi::ThermalState::Nominal => ThermalState::Nominal,
        ffi::ThermalState::Fair => ThermalState::Fair,
        ffi::ThermalState::Serious => ThermalState::Serious,
        ffi::ThermalState::Critical => ThermalState::Critical,
        ffi::ThermalState::Unknown => ThermalState::Unknown,
    }
}

pub fn get_system_load() -> SystemLoad {
    let load = ffi::get_apple_system_load();
    SystemLoad {
        cpu_usage: load.cpu_usage,
        memory_used: load.memory_used,
        memory_total: load.memory_total,
    }
}
