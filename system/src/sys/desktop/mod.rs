use crate::{ConnectivityInfo, ConnectionType, SystemLoad, ThermalState};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System, Networks};

pub fn get_connectivity_info() -> ConnectivityInfo {
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
        if name_lower.contains("wlan") || name_lower.contains("wi-fi") 
            || name_lower.contains("wifi") || name_lower.starts_with("en") && !name_lower.contains("ethernet") {
            connection_type = ConnectionType::Wifi;
            break;
        } else if name_lower.contains("eth") || name_lower.contains("ethernet") 
            || name_lower.starts_with("enp") || name_lower.starts_with("eno") {
            connection_type = ConnectionType::Ethernet;
        } else if name_lower.contains("wwan") || name_lower.contains("cellular") {
            connection_type = ConnectionType::Cellular;
            break;
        } else if name_lower.contains("vpn") || name_lower.contains("tun") || name_lower.contains("tap") {
            connection_type = ConnectionType::Vpn;
        } else if name_lower.contains("bluetooth") || name_lower.contains("pan") {
            connection_type = ConnectionType::Bluetooth;
        } else if connection_type == ConnectionType::None {
            connection_type = ConnectionType::Other;
        }
    }

    ConnectivityInfo {
        connection_type,
        is_connected: has_connection && connection_type != ConnectionType::None,
    }
}

pub fn get_thermal_state() -> ThermalState {
    use sysinfo::Components;
    let components = Components::new_with_refreshed_list();
    
    // Very simple heuristic: check max component temp
    let mut max_temp = 0.0f32;
    for component in &components {
        let temp = component.temperature();
        if temp > max_temp {
            max_temp = temp;
        }
    }
    
    if max_temp > 90.0 {
        ThermalState::Critical
    } else if max_temp > 80.0 {
        ThermalState::Serious
    } else if max_temp > 70.0 {
        ThermalState::Fair
    } else {
        ThermalState::Nominal
    }
}

pub fn get_system_load() -> SystemLoad {
    let mut system = System::new_with_specifics(
        RefreshKind::new()
        .with_cpu(CpuRefreshKind::everything())
        .with_memory(MemoryRefreshKind::everything()),
    );
    // Refresh twice for CPU usage calculation if needed, 
    // but sysinfo usually needs a delay between refreshes for accurate CPU usage.
    // For a oneshot call, this might return 0.0 for CPU.
    // A proper implementation might need a background thread or stateful object.
    // For simplicity here, we'll just read what we can.
    std::thread::sleep(System::MINIMUM_CPU_UPDATE_INTERVAL);
    system.refresh_cpu();
    system.refresh_memory();

    let cpu_usage = system.global_cpu_info().cpu_usage();
    let memory_used = system.used_memory();
    let memory_total = system.total_memory();

    SystemLoad {
        cpu_usage,
        memory_used,
        memory_total,
    }
}
