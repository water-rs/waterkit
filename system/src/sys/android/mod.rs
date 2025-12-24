use crate::{ConnectivityInfo, ConnectionType, SystemLoad, ThermalState};
use jni::objects::{JObject, JValue, GlobalRef};
use jni::{JNIEnv, JavaVM};
use std::sync::OnceLock;

static JAVA_VM: OnceLock<JavaVM> = OnceLock::new();
static CONTEXT: OnceLock<GlobalRef> = OnceLock::new();

/// Initialize the system module with Android context.
/// Must be called from Java/Kotlin before using system info functions.
pub fn init(env: &mut JNIEnv, context: JObject) {
    let vm = env.get_java_vm().expect("Failed to get JavaVM");
    let _ = JAVA_VM.set(vm);

    let global_ctx = env.new_global_ref(context).expect("Failed to create global ref");
    let _ = CONTEXT.set(global_ctx);
}

fn with_jni<T, F>(f: F) -> Option<T>
where
    F: FnOnce(&mut JNIEnv, &JObject) -> Option<T>,
{
    let vm = JAVA_VM.get()?;
    let ctx = CONTEXT.get()?;

    let mut env = vm.attach_current_thread().ok()?;
    f(&mut env, ctx.as_obj())
}

pub fn get_connectivity_info() -> ConnectivityInfo {
    let result = with_jni(|env, ctx| {
        let class = env.find_class("com/waterkit/system/SystemHelper").ok()?;
        let result = env.call_static_method(
            class,
            "getConnectivity",
            "(Landroid/content/Context;)I",
            &[JValue::Object(ctx)],
        ).ok()?.i().ok()?;
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

    ConnectivityInfo {
        connection_type,
        is_connected: !matches!(result.unwrap_or(0), 0),
    }
}

pub fn get_thermal_state() -> ThermalState {
    let result = with_jni(|env, ctx| {
        let class = env.find_class("com/waterkit/system/SystemHelper").ok()?;
        let result = env.call_static_method(
            class,
            "getThermalState",
            "(Landroid/content/Context;)I",
            &[JValue::Object(ctx)],
        ).ok()?.i().ok()?;
        Some(result)
    });

    // Android thermal statuses map: 0=None, 1=Light, 2=Moderate, 3=Severe, 4=Critical, 5=Emergency, 6=Shutdown
    match result.unwrap_or(-1) {
        0 => ThermalState::Nominal,
        1 => ThermalState::Fair,
        2 => ThermalState::Fair,
        3 => ThermalState::Serious,
        4 | 5 | 6 => ThermalState::Critical,
        _ => ThermalState::Unknown,
    }
}

pub fn get_system_load() -> SystemLoad {
    let result = with_jni(|env, ctx| {
        let class = env.find_class("com/waterkit/system/SystemHelper").ok()?;
        let load_info = env.call_static_method(
            class,
            "getSystemLoad",
            "(Landroid/content/Context;)Lcom/waterkit/system/SystemHelper$LoadInfo;",
            &[JValue::Object(ctx)],
        ).ok()?.l().ok()?;

        let cpu = env.get_field(&load_info, "cpu", "F").ok()?.f().ok()?;
        let mem_used = env.get_field(&load_info, "memUsed", "J").ok()?.j().ok()?;
        let mem_total = env.get_field(&load_info, "memTotal", "J").ok()?.j().ok()?;

        Some((cpu, mem_used as u64, mem_total as u64))
    });

    match result {
        Some((cpu, mem_used, mem_total)) => SystemLoad {
            cpu_usage: cpu,
            memory_used: mem_used,
            memory_total: mem_total,
        },
        None => SystemLoad {
            cpu_usage: 0.0,
            memory_used: 0,
            memory_total: 0,
        },
    }
}

// JNI export for initialization from Java/Kotlin
#[no_mangle]
pub extern "system" fn Java_com_waterkit_system_SystemBridge_nativeInit<'local>(
    mut env: JNIEnv<'local>,
    _class: jni::objects::JClass<'local>,
    context: JObject<'local>,
) {
    init(&mut env, context);
}
