use crate::{ConnectionType, ConnectivityInfo, SystemLoad, ThermalState};
use jni::objects::{JObject, JValue};
use jni::{Env, jni_sig, jni_str};
use waterkit_build::{AndroidError, with_android_context};

/// Runs `f` with the calling thread attached to the application's JVM and the
/// Android `Context`.
///
/// System probes report "unknown" rather than failing, so an unavailable JVM and
/// a JNI failure collapse to the same `None`.
fn with_jni<T, F>(f: F) -> Option<T>
where
    F: FnOnce(&mut Env<'_>, &JObject<'_>) -> Option<T>,
{
    with_android_context(|env, context| -> Result<Option<T>, AndroidError> { Ok(f(env, context)) })
        .ok()
        .flatten()
}

pub fn get_connectivity_info() -> ConnectivityInfo {
    let result = with_jni(|env, context| {
        env.call_static_method(
            jni_str!("com/waterkit/system/SystemHelper"),
            jni_str!("getConnectivity"),
            jni_sig!("(Landroid/content/Context;)I"),
            &[JValue::Object(context)],
        )
        .ok()?
        .i()
        .ok()
    });

    let connection_type = match result.unwrap_or(0) {
        1 => ConnectionType::Wifi,
        2 => ConnectionType::Cellular,
        3 => ConnectionType::Ethernet,
        4 => ConnectionType::Bluetooth,
        5 => ConnectionType::Vpn,
        6 => ConnectionType::Other,
        _ => ConnectionType::None,
    };

    ConnectivityInfo::new(connection_type, !matches!(result.unwrap_or(0), 0))
}

pub fn get_thermal_state() -> ThermalState {
    let result = with_jni(|env, context| {
        env.call_static_method(
            jni_str!("com/waterkit/system/SystemHelper"),
            jni_str!("getThermalState"),
            jni_sig!("(Landroid/content/Context;)I"),
            &[JValue::Object(context)],
        )
        .ok()?
        .i()
        .ok()
    });

    // Android thermal statuses map: 0=None, 1=Light, 2=Moderate, 3=Severe, 4=Critical, 5=Emergency, 6=Shutdown
    match result.unwrap_or(-1) {
        0 => ThermalState::Nominal,
        1 | 2 => ThermalState::Fair,
        3 => ThermalState::Serious,
        4..=6 => ThermalState::Critical,
        _ => ThermalState::Unknown,
    }
}

pub fn get_system_load() -> SystemLoad {
    let result = with_jni(|env, context| {
        let load_info = env
            .call_static_method(
                jni_str!("com/waterkit/system/SystemHelper"),
                jni_str!("getSystemLoad"),
                jni_sig!("(Landroid/content/Context;)Lcom/waterkit/system/SystemHelper$LoadInfo;"),
                &[JValue::Object(context)],
            )
            .ok()?
            .l()
            .ok()?;

        let cpu = env
            .get_field(&load_info, jni_str!("cpu"), jni_sig!("F"))
            .ok()?
            .f()
            .ok()?;
        let mem_used = env
            .get_field(&load_info, jni_str!("memUsed"), jni_sig!("J"))
            .ok()?
            .j()
            .ok()?;
        let mem_total = env
            .get_field(&load_info, jni_str!("memTotal"), jni_sig!("J"))
            .ok()?
            .j()
            .ok()?;

        Some((
            cpu,
            u64::try_from(mem_used).ok()?,
            u64::try_from(mem_total).ok()?,
        ))
    });

    match result {
        Some((cpu, mem_used, mem_total)) => SystemLoad::new(cpu, mem_used, mem_total),
        None => SystemLoad::new(0.0, 0, 0),
    }
}
