use crate::{ConnectionType, ConnectivityInfo, SystemLoad, ThermalState};
use jni::JNIEnv;
use jni::objects::{JObject, JValue};

fn with_jni<T, F>(f: F) -> Option<T>
where
    F: FnOnce(&mut JNIEnv, JObject<'_>) -> Option<T>,
{
    let android_ctx = ndk_context::android_context();
    let vm = unsafe { jni::JavaVM::from_raw(android_ctx.vm().cast()).ok()? };
    let context = unsafe { JObject::from_raw(android_ctx.context().cast()) };
    let mut env = vm.attach_current_thread().ok()?;
    f(&mut env, context)
}

pub fn get_connectivity_info() -> ConnectivityInfo {
    let result = with_jni(|env, ctx| {
        let class = env.find_class("com/waterkit/system/SystemHelper").ok()?;
        let result = env
            .call_static_method(
                class,
                "getConnectivity",
                "(Landroid/content/Context;)I",
                &[JValue::Object(&ctx)],
            )
            .ok()?
            .i()
            .ok()?;
        Some(result)
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
    let result = with_jni(|env, ctx| {
        let class = env.find_class("com/waterkit/system/SystemHelper").ok()?;
        let result = env
            .call_static_method(
                class,
                "getThermalState",
                "(Landroid/content/Context;)I",
                &[JValue::Object(&ctx)],
            )
            .ok()?
            .i()
            .ok()?;
        Some(result)
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
    let result = with_jni(|env, ctx| {
        let class = env.find_class("com/waterkit/system/SystemHelper").ok()?;
        let load_info = env
            .call_static_method(
                class,
                "getSystemLoad",
                "(Landroid/content/Context;)Lcom/waterkit/system/SystemHelper$LoadInfo;",
                &[JValue::Object(&ctx)],
            )
            .ok()?
            .l()
            .ok()?;

        let cpu = env.get_field(&load_info, "cpu", "F").ok()?.f().ok()?;
        let mem_used = env.get_field(&load_info, "memUsed", "J").ok()?.j().ok()?;
        let mem_total = env.get_field(&load_info, "memTotal", "J").ok()?.j().ok()?;

        Some((cpu, mem_used as u64, mem_total as u64))
    });

    match result {
        Some((cpu, mem_used, mem_total)) => SystemLoad::new(cpu, mem_used, mem_total),
        None => SystemLoad::new(0.0, 0, 0),
    }
}
