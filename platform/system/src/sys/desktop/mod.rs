#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
use linux as os;
#[cfg(target_os = "windows")]
use windows as os;

use crate::{ConnectionType, ConnectivityInfo, SystemError, SystemLoad, ThermalState};
use sysinfo::{
    Components, CpuRefreshKind, MINIMUM_CPU_UPDATE_INTERVAL, MemoryRefreshKind, RefreshKind, System,
};

#[expect(
    clippy::unused_async,
    reason = "the public signature is async on every platform; this one resolves synchronously"
)]
pub async fn connectivity() -> Result<ConnectivityInfo, SystemError> {
    let transport = os::transport()?;
    Ok(ConnectivityInfo::new(
        transport,
        transport != ConnectionType::None,
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

pub async fn load() -> Result<SystemLoad, SystemError> {
    let mut system = System::new_with_specifics(
        RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::everything())
            .with_memory(MemoryRefreshKind::everything()),
    );
    // CPU usage is the difference between two samples taken at least
    // `MINIMUM_CPU_UPDATE_INTERVAL` apart. `system` is `Send`, so the
    // future stays `Send` across the timer.
    futures_timer::Delay::new(MINIMUM_CPU_UPDATE_INTERVAL).await;
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
