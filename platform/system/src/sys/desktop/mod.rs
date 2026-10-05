use crate::{ConnectionType, ConnectivityInfo, SystemError, SystemLoad, ThermalState};
use sysinfo::{
    Components, CpuRefreshKind, MINIMUM_CPU_UPDATE_INTERVAL, MemoryRefreshKind, Networks,
    RefreshKind, System,
};

#[expect(
    clippy::unnecessary_wraps,
    reason = "every platform shares the fallible signature; sysinfo reports no failure here"
)]
pub fn connectivity() -> Result<ConnectivityInfo, SystemError> {
    let networks = Networks::new_with_refreshed_list();

    let mut has_connection = false;
    let mut connection_type = ConnectionType::None;

    for (name, _data) in &networks {
        let name_lower = name.to_lowercase();

        // Skip loopback
        if name_lower.contains("lo") || name_lower.contains("loopback") {
            continue;
        }

        has_connection = true;

        // Identify interface type by name
        if name_lower.contains("wlan")
            || name_lower.contains("wi-fi")
            || name_lower.contains("wifi")
            || name_lower.starts_with("en") && !name_lower.contains("ethernet")
        {
            connection_type = ConnectionType::Wifi;
            break;
        } else if name_lower.contains("eth")
            || name_lower.contains("ethernet")
            || name_lower.starts_with("enp")
            || name_lower.starts_with("eno")
        {
            connection_type = ConnectionType::Ethernet;
        } else if name_lower.contains("wwan") || name_lower.contains("cellular") {
            connection_type = ConnectionType::Cellular;
            break;
        } else if name_lower.contains("vpn")
            || name_lower.contains("tun")
            || name_lower.contains("tap")
        {
            connection_type = ConnectionType::Vpn;
        } else if name_lower.contains("bluetooth") || name_lower.contains("pan") {
            connection_type = ConnectionType::Bluetooth;
        } else if connection_type == ConnectionType::None {
            connection_type = ConnectionType::Other;
        }
    }

    Ok(ConnectivityInfo::new(
        connection_type,
        has_connection && connection_type != ConnectionType::None,
    ))
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "every platform shares the fallible signature; sysinfo reports no failure here"
)]
pub fn thermal_state() -> Result<Option<ThermalState>, SystemError> {
    // Very simple heuristic: the hottest component's temperature. A machine
    // whose components report no temperature has no thermal state to derive.
    let components = Components::new_with_refreshed_list();
    let Some(max_temp) = components
        .iter()
        .filter_map(sysinfo::Component::temperature)
        .reduce(f32::max)
    else {
        return Ok(None);
    };

    Ok(Some(if max_temp > 90.0 {
        ThermalState::Critical
    } else if max_temp > 80.0 {
        ThermalState::Serious
    } else if max_temp > 70.0 {
        ThermalState::Fair
    } else {
        ThermalState::Nominal
    }))
}

pub fn load() -> Result<SystemLoad, SystemError> {
    let mut system = System::new_with_specifics(
        RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::everything())
            .with_memory(MemoryRefreshKind::everything()),
    );
    // CPU usage is the difference between two samples taken at least
    // `MINIMUM_CPU_UPDATE_INTERVAL` apart.
    std::thread::sleep(MINIMUM_CPU_UPDATE_INTERVAL);
    system.refresh_cpu_all();
    system.refresh_memory();

    // sysinfo reports what it cannot read as empty or zero.
    if system.cpus().is_empty() {
        return Err(SystemError::Platform(String::from(
            "sysinfo could not read CPU statistics",
        )));
    }
    let memory_total = system.total_memory();
    if memory_total == 0 {
        return Err(SystemError::Platform(String::from(
            "sysinfo could not read memory statistics",
        )));
    }

    Ok(SystemLoad::new(
        Some(system.global_cpu_usage()),
        system.used_memory(),
        memory_total,
    ))
}
