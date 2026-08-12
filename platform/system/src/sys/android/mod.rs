use crate::{ConnectionType, ConnectivityInfo, SystemLoad, ThermalState};
use jni::objects::{JObject, JValue};
use jni::{Env, JavaVM, jni_sig, jni_str};

/// Runs `f` with the calling thread attached to the application's JVM and the
/// Android `Context` published by `ndk_context`.
fn with_jni<T, F>(f: F) -> Option<T>
where
    F: FnOnce(&mut Env<'_>, &JObject<'_>) -> Option<T>,
{
    let android_context = ndk_context::android_context();
    let raw_vm: *mut jni::sys::JavaVM = android_context.vm().cast();
    let raw_context: jni::sys::jobject = android_context.context().cast();
    assert!(
        !raw_vm.is_null(),
        "waterkit-system: ndk_context returned a null JavaVM"
    );
    assert!(
        !raw_context.is_null(),
        "waterkit-system: ndk_context returned a null Android Context"
    );

    // SAFETY: `ndk_context` publishes the process' JavaVM pointer, which stays
    // valid for the lifetime of the application.
    let vm = unsafe { JavaVM::from_raw(raw_vm) };
    vm.attach_current_thread(|env| -> jni::errors::Result<Option<T>> {
        // SAFETY: `ndk_context` publishes a global reference to the application
        // `Context` that outlives this attachment, and `as_cast_raw` only
        // borrows it.
        let context = unsafe { env.as_cast_raw::<JObject>(&raw_context)? };
        Ok(f(env, &context))
    })
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
