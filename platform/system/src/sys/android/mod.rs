use crate::{ConnectionType, ConnectivityInfo, SystemError, SystemLoad, ThermalState};
use jni::objects::{JObject, JValue, JValueOwned};
use jni::signature::MethodSignature;
use jni::strings::JNIStr;
use jni::{Env, jni_sig, jni_str};
use waterkit_build::{
    AndroidError, DexHelper, describe_jni_error, dex_helper, with_android_context,
};

/// `com.waterkit.system.SystemHelper`, compiled into the app's DEX by the
/// packager and resolved through the application's `ClassLoader`.
static HELPER: DexHelper = dex_helper!("com.waterkit.system.SystemHelper");

impl From<AndroidError> for SystemError {
    fn from(error: AndroidError) -> Self {
        Self::Platform(error.to_string())
    }
}

/// Builds the error for the JNI call `call` failing with `error`, carrying the
/// Java exception it threw.
fn jni_error(env: &Env<'_>, call: &str, error: jni::errors::Error) -> SystemError {
    SystemError::Platform(format!("{call}: {}", describe_jni_error(env, error)))
}

/// Calls the static `SystemHelper.<method>(Context)`.
fn call_helper<'local>(
    env: &mut Env<'local>,
    context: &JObject<'_>,
    method: &'static JNIStr,
    signature: &MethodSignature<'_, '_>,
) -> Result<JValueOwned<'local>, SystemError> {
    let helper = HELPER
        .class(env, context)
        .map_err(|error| SystemError::Platform(format!("SystemHelper.{method}: {error}")))?;
    env.call_static_method(helper, method, signature, &[JValue::Object(context)])
        .map_err(|error| jni_error(env, &format!("SystemHelper.{method}"), error))
}

/// Calls a `SystemHelper` method that returns an object, failing on `null`.
fn call_helper_object<'local>(
    env: &mut Env<'local>,
    context: &JObject<'_>,
    method: &'static JNIStr,
    signature: &MethodSignature<'_, '_>,
) -> Result<Option<JObject<'local>>, SystemError> {
    let value = call_helper(env, context, method, signature)?;
    let object = value
        .l()
        .map_err(|error| jni_error(env, &format!("SystemHelper.{method}"), error))?;
    Ok((!object.is_null()).then_some(object))
}

pub fn connectivity() -> Result<ConnectivityInfo, SystemError> {
    let transport = with_android_context(|env, context| {
        let value = call_helper(
            env,
            context,
            jni_str!("getConnectivity"),
            &jni_sig!("(Landroid/content/Context;)I"),
        )?;
        value
            .i()
            .map_err(|error| jni_error(env, "SystemHelper.getConnectivity", error))
    })?;

    let connection_type = match transport {
        0 => ConnectionType::None,
        1 => ConnectionType::Wifi,
        2 => ConnectionType::Cellular,
        3 => ConnectionType::Ethernet,
        4 => ConnectionType::Bluetooth,
        5 => ConnectionType::Vpn,
        6 => ConnectionType::Other,
        other => {
            return Err(SystemError::Platform(format!(
                "SystemHelper.getConnectivity returned unknown transport {other}"
            )));
        }
    };
    Ok(ConnectivityInfo::new(
        connection_type,
        connection_type != ConnectionType::None,
    ))
}

pub fn thermal_state() -> Result<Option<ThermalState>, SystemError> {
    let status = with_android_context(|env, context| {
        let Some(status) = call_helper_object(
            env,
            context,
            jni_str!("getThermalState"),
            &jni_sig!("(Landroid/content/Context;)Ljava/lang/Integer;"),
        )?
        else {
            return Ok(None);
        };
        env.call_method(&status, jni_str!("intValue"), jni_sig!("()I"), &[])
            .and_then(JValueOwned::i)
            .map(Some)
            .map_err(|error| jni_error(env, "Integer.intValue", error))
    })?;

    // `PowerManager.THERMAL_STATUS_*`.
    status
        .map(|status| match status {
            0 => Ok(ThermalState::Nominal),
            1 | 2 => Ok(ThermalState::Fair),
            3 => Ok(ThermalState::Serious),
            4..=6 => Ok(ThermalState::Critical),
            other => Err(SystemError::Platform(format!(
                "PowerManager reported unknown thermal status {other}"
            ))),
        })
        .transpose()
}

pub fn load() -> Result<SystemLoad, SystemError> {
    let (used, total) = with_android_context(|env, context| {
        let memory = call_helper_object(
            env,
            context,
            jni_str!("getMemoryLoad"),
            &jni_sig!("(Landroid/content/Context;)Lcom/waterkit/system/SystemHelper$MemoryLoad;"),
        )?
        .ok_or_else(|| {
            SystemError::Platform(String::from("SystemHelper.getMemoryLoad returned null"))
        })?;
        let mut field = |name: &'static JNIStr| {
            env.get_field(&memory, name, jni_sig!("J"))
                .and_then(JValueOwned::j)
                .map_err(|error| jni_error(env, &format!("SystemHelper$MemoryLoad.{name}"), error))
        };
        Ok::<_, SystemError>((field(jni_str!("used"))?, field(jni_str!("total"))?))
    })?;

    let bytes = |value: i64, what: &str| {
        u64::try_from(value).map_err(|_| {
            SystemError::Platform(format!(
                "ActivityManager reported negative {what} memory {value}"
            ))
        })
    };
    // Android does not expose system-wide CPU statistics to applications:
    // `/proc/stat` has been closed to them since Android 8.
    Ok(SystemLoad::new(
        None,
        bytes(used, "used")?,
        bytes(total, "total")?,
    ))
}
