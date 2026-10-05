use crate::{ConnectionType, ConnectivityInfo, SystemError, SystemLoad, ThermalState};

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

    #[swift_bridge(swift_repr = "struct")]
    pub struct RustConnectivityInfo {
        pub connection_type: ConnectionType,
        pub is_connected: bool,
    }

    pub enum ConnectivityResult {
        Reported(RustConnectivityInfo),
        Failed(String),
    }

    #[swift_bridge(swift_repr = "struct")]
    pub struct RustSystemLoad {
        pub cpu_usage: f32,
        pub memory_used: u64,
        pub memory_total: u64,
    }

    pub enum SystemLoadResult {
        Measured(RustSystemLoad),
        Failed(String),
    }

    extern "Swift" {
        fn get_apple_connectivity() -> ConnectivityResult;
        fn get_apple_thermal_state() -> isize;
        fn get_apple_system_load() -> SystemLoadResult;
    }
}

pub fn connectivity() -> Result<ConnectivityInfo, SystemError> {
    let info = match ffi::get_apple_connectivity() {
        ffi::ConnectivityResult::Reported(info) => info,
        ffi::ConnectivityResult::Failed(message) => return Err(SystemError::Platform(message)),
    };
    let connection_type = match info.connection_type {
        ffi::ConnectionType::Wifi => ConnectionType::Wifi,
        ffi::ConnectionType::Cellular => ConnectionType::Cellular,
        ffi::ConnectionType::Ethernet => ConnectionType::Ethernet,
        ffi::ConnectionType::Bluetooth => ConnectionType::Bluetooth,
        ffi::ConnectionType::Vpn => ConnectionType::Vpn,
        ffi::ConnectionType::Other => ConnectionType::Other,
        ffi::ConnectionType::None => ConnectionType::None,
    };
    Ok(ConnectivityInfo::new(connection_type, info.is_connected))
}

pub fn thermal_state() -> Result<Option<ThermalState>, SystemError> {
    // `ProcessInfo.ThermalState` raw values.
    match ffi::get_apple_thermal_state() {
        0 => Ok(Some(ThermalState::Nominal)),
        1 => Ok(Some(ThermalState::Fair)),
        2 => Ok(Some(ThermalState::Serious)),
        3 => Ok(Some(ThermalState::Critical)),
        other => Err(SystemError::Platform(format!(
            "ProcessInfo reported unknown thermal state {other}"
        ))),
    }
}

pub fn load() -> Result<SystemLoad, SystemError> {
    match ffi::get_apple_system_load() {
        ffi::SystemLoadResult::Measured(load) => Ok(SystemLoad::new(
            Some(load.cpu_usage),
            load.memory_used,
            load.memory_total,
        )),
        ffi::SystemLoadResult::Failed(message) => Err(SystemError::Platform(message)),
    }
}
