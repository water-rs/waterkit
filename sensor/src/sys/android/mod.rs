//! Android sensor implementation using JNI.

use crate::{ScalarData, SensorData, SensorError, SensorStream};
use futures::stream;
use jni::objects::{GlobalRef, JObject, JValue};
use jni::{JNIEnv, JavaVM};
use std::sync::OnceLock;

/// Embedded DEX bytecode containing SensorHelper class.
static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

/// Cached class loader for the embedded DEX.
static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();
/// Global reference to the Android Context.
static GLOBAL_CONTEXT: OnceLock<GlobalRef> = OnceLock::new();
/// Global reference to the Java VM.
static JAVA_VM: OnceLock<JavaVM> = OnceLock::new();

/// Initialize the sensor subsystem with a Context.
/// This must be called before using any sensor APIs on Android.
pub fn init(env: &mut JNIEnv, context: &JObject) -> Result<(), SensorError> {
    if GLOBAL_CONTEXT.get().is_some() {
        return Ok(());
    }

    // Initialize DEX loader
    init_with_context(env, context)?;

    // Store JavaVM
    if JAVA_VM.get().is_none() {
        let vm = env
            .get_java_vm()
            .map_err(|e| SensorError::Unknown(format!("get_java_vm failed: {e}")))?;
        let _ = JAVA_VM.set(vm);
    }

    // Store Context
    let context_ref = env
        .new_global_ref(context)
        .map_err(|e| SensorError::Unknown(format!("new_global_ref context failed: {e}")))?;
    let _ = GLOBAL_CONTEXT.set(context_ref);

    Ok(())
}

/// Initialize the DEX class loader (internal).
fn init_with_context(env: &mut JNIEnv, context: &JObject) -> Result<(), SensorError> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }

    let cache_dir = env
        .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])
        .map_err(|e| SensorError::Unknown(format!("getCacheDir failed: {e}")))?
        .l()
        .map_err(|e| SensorError::Unknown(format!("getCacheDir result: {e}")))?;

    let cache_path = env
        .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .map_err(|e| SensorError::Unknown(format!("getAbsolutePath failed: {e}")))?
        .l()
        .map_err(|e| SensorError::Unknown(format!("getAbsolutePath result: {e}")))?;

    let dex_path = format!(
        "{}/waterkit_sensor.dex",
        env.get_string((&cache_path).into())
            .map_err(|e| SensorError::Unknown(format!("get_string failed: {e}")))?
            .to_str()
            .map_err(|e| SensorError::Unknown(format!("to_str failed: {e}")))?
    );
    
    // Remove if exists to handle previous read-only setting
    let _ = std::fs::remove_file(&dex_path);

    log::info!("Initializing DEX loader with path: {}", dex_path);
    std::fs::write(&dex_path, DEX_BYTES)
        .map_err(|e| SensorError::Unknown(format!("write DEX failed: {e}")))?;

    // Make DEX read-only as required by modern Android security
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dex_path)
            .map_err(|e| SensorError::Unknown(format!("metadata DEX failed: {e}")))?
            .permissions();
        perms.set_mode(0o444); // Read-only
        std::fs::set_permissions(&dex_path, perms)
            .map_err(|e| SensorError::Unknown(format!("set_permissions DEX failed: {e}")))?;
        log::info!("DEX file permissions set to read-only (0444)");
    }

    let dex_path_jstring = env
        .new_string(&dex_path)
        .map_err(|e| SensorError::Unknown(format!("new_string failed: {e}")))?;

    log::info!("Creating DexClassLoader...");
    let parent_loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .map_err(|e| SensorError::Unknown(format!("getClassLoader failed: {e}")))?
        .l()
        .map_err(|e| SensorError::Unknown(format!("getClassLoader result: {e}")))?;

    let dex_class_loader_class = env
        .find_class("dalvik/system/DexClassLoader")
        .map_err(|e| SensorError::Unknown(format!("find DexClassLoader: {e}")))?;

    let class_loader = env
        .new_object(
            dex_class_loader_class,
            "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/ClassLoader;)V",
            &[
                JValue::Object(&dex_path_jstring),
                JValue::Object(&cache_path),
                JValue::Object(&JObject::null()),
                JValue::Object(&parent_loader),
            ],
        )
        .map_err(|e| {
            log::error!("new DexClassLoader failed: {}", e);
            SensorError::Unknown(format!("new DexClassLoader: {e}"))
        })?;

    log::info!("DexClassLoader created successfully.");
    let global_ref = env
        .new_global_ref(class_loader)
        .map_err(|e| SensorError::Unknown(format!("new_global_ref: {e}")))?;

    let _ = CLASS_LOADER.set(global_ref);
    Ok(())
}

fn load_helper_class<'a>(env: &mut JNIEnv<'a>) -> Result<jni::objects::JClass<'a>, SensorError> {
    let class_loader = CLASS_LOADER
        .get()
        .ok_or_else(|| SensorError::Unknown("Class loader not initialized".into()))?;

    let helper_class_name = env
        .new_string("waterkit.sensor.SensorHelper")
        .map_err(|e| SensorError::Unknown(format!("new_string: {e}")))?;

    let helper_class = env
        .call_method(
            class_loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&helper_class_name)],
        )
        .map_err(|e| SensorError::Unknown(format!("loadClass: {e}")))?
        .l()
        .map_err(|e| SensorError::Unknown(format!("loadClass result: {e}")))?;

    Ok(helper_class.into())
}

fn get_env_and_context() -> Result<(jni::AttachGuard<'static>, JObject<'static>), SensorError> {
    let vm = JAVA_VM
        .get()
        .ok_or_else(|| SensorError::Unknown("JavaVM not initialized. Call init() first.".into()))?;
    let context_ref = GLOBAL_CONTEXT.get().ok_or_else(|| {
        SensorError::Unknown("Context not initialized. Call init() first.".into())
    })?;

    let env = vm
        .attach_current_thread()
        .map_err(|e| SensorError::Unknown(format!("attach_current_thread failed: {e}")))?;

    let context = context_ref.as_obj();
    let local_ref = env
        .new_local_ref(context)
        .map_err(|e| SensorError::Unknown(format!("new_local_ref failed: {e}")))?;
    Ok((env, local_ref))
}

fn parse_sensor_result(env: &mut JNIEnv, result: JObject) -> Result<SensorData, SensorError> {
    let arr: jni::objects::JDoubleArray = result.into();
    let len =
        env.get_array_length(&arr)
            .map_err(|e| SensorError::Unknown(format!("get_array_length: {e}")))? as usize;

    if len < 1 {
        return Err(SensorError::NotAvailable);
    }

    let mut buf = vec![0.0f64; len];
    env.get_double_array_region(&arr, 0, &mut buf)
        .map_err(|e| SensorError::Unknown(format!("get_double_array_region: {e}")))?;

    if buf[0] < 0.5 {
        return Err(SensorError::NotAvailable);
    }

    if len < 5 {
        return Err(SensorError::Unknown("Invalid result array".into()));
    }

    Ok(SensorData {
        x: buf[1],
        y: buf[2],
        z: buf[3],
        timestamp: buf[4] as u64,
    })
}

fn parse_scalar_result(env: &mut JNIEnv, result: JObject) -> Result<ScalarData, SensorError> {
    let arr: jni::objects::JDoubleArray = result.into();
    let len =
        env.get_array_length(&arr)
            .map_err(|e| SensorError::Unknown(format!("get_array_length: {e}")))? as usize;

    if len < 1 {
        return Err(SensorError::NotAvailable);
    }

    let mut buf = vec![0.0f64; len];
    env.get_double_array_region(&arr, 0, &mut buf)
        .map_err(|e| SensorError::Unknown(format!("get_double_array_region: {e}")))?;

    if buf[0] < 0.5 {
        return Err(SensorError::NotAvailable);
    }

    if len < 3 {
        return Err(SensorError::Unknown("Invalid result array".into()));
    }

    Ok(ScalarData {
        value: buf[1],
        timestamp: buf[2] as u64,
    })
}

// Check sensor availability with manual context (helper)
pub fn is_sensor_available_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    sensor_type: i32,
) -> Result<bool, SensorError> {
    log::info!("Checking sensor availability for type {}...", sensor_type);
    init_with_context(env, context)?;
    log::info!("Loading helper class...");
    let helper = load_helper_class(env)?;
    log::info!("Calling isSensorAvailable static method...");

    let result = env
        .call_static_method(
            helper,
            "isSensorAvailable",
            "(Landroid/content/Context;I)Z",
            &[JValue::Object(context), JValue::Int(sensor_type)],
        )
        .map_err(|e| {
            log::error!("isSensorAvailable failed: {}", e);
            SensorError::Unknown(format!("isSensorAvailable: {e}"))
        })?
        .z()
        .map_err(|e| SensorError::Unknown(format!("isSensorAvailable result: {e}")))?;

    log::info!("Sensor available: {}", result);
    Ok(result)
}

// Read sensor with manual context (helper)
pub fn read_sensor_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    sensor_type: i32,
) -> Result<SensorData, SensorError> {
    init_with_context(env, context)?;
    let helper = load_helper_class(env)?;

    let result = env
        .call_static_method(
            helper,
            "readSensor",
            "(Landroid/content/Context;I)[D",
            &[JValue::Object(context), JValue::Int(sensor_type)],
        )
        .map_err(|e| SensorError::Unknown(format!("readSensor: {e}")))?
        .l()
        .map_err(|e| SensorError::Unknown(format!("readSensor result: {e}")))?;

    parse_sensor_result(env, result)
}

// Read pressure with manual context (helper)
pub fn read_pressure_with_context(
    env: &mut JNIEnv,
    context: &JObject,
) -> Result<ScalarData, SensorError> {
    init_with_context(env, context)?;
    let helper = load_helper_class(env)?;

    let result = env
        .call_static_method(
            helper,
            "readPressure",
            "(Landroid/content/Context;)[D",
            &[JValue::Object(context)],
        )
        .map_err(|e| SensorError::Unknown(format!("readPressure: {e}")))?
        .l()
        .map_err(|e| SensorError::Unknown(format!("readPressure result: {e}")))?;

    parse_scalar_result(env, result)
}

// Read light with manual context (helper)
pub fn read_light_with_context(
    env: &mut JNIEnv,
    context: &JObject,
) -> Result<ScalarData, SensorError> {
    init_with_context(env, context)?;
    let helper = load_helper_class(env)?;

    let result = env
        .call_static_method(
            helper,
            "readLight",
            "(Landroid/content/Context;)[D",
            &[JValue::Object(context)],
        )
        .map_err(|e| SensorError::Unknown(format!("readLight: {e}")))?
        .l()
        .map_err(|e| SensorError::Unknown(format!("readLight result: {e}")))?;

    parse_scalar_result(env, result)
}

// --- Parameter-less API Implementation using Global Context ---

pub fn accelerometer_available() -> bool {
    if let Ok((mut env, context)) = get_env_and_context() {
        is_sensor_available_with_context(&mut env, &context, 1).unwrap_or(false)
    } else {
        false
    }
}

pub async fn accelerometer_read() -> Result<SensorData, SensorError> {
    let (mut env, context) = get_env_and_context()?;
    read_sensor_with_context(&mut env, &context, 1)
}

pub fn accelerometer_watch(interval_ms: u32) -> Result<SensorStream<SensorData>, SensorError> {
    if !accelerometer_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(Box::pin(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        match accelerometer_read().await {
            Ok(data) => Some((data, ())),
            _ => None,
        }
    })))
}

pub fn gyroscope_available() -> bool {
    if let Ok((mut env, context)) = get_env_and_context() {
        is_sensor_available_with_context(&mut env, &context, 4).unwrap_or(false)
    } else {
        false
    }
}

pub async fn gyroscope_read() -> Result<SensorData, SensorError> {
    let (mut env, context) = get_env_and_context()?;
    read_sensor_with_context(&mut env, &context, 4)
}

pub fn gyroscope_watch(interval_ms: u32) -> Result<SensorStream<SensorData>, SensorError> {
    if !gyroscope_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(Box::pin(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        match gyroscope_read().await {
            Ok(data) => Some((data, ())),
            _ => None,
        }
    })))
}

pub fn magnetometer_available() -> bool {
    if let Ok((mut env, context)) = get_env_and_context() {
        is_sensor_available_with_context(&mut env, &context, 2).unwrap_or(false)
    } else {
        false
    }
}

pub async fn magnetometer_read() -> Result<SensorData, SensorError> {
    let (mut env, context) = get_env_and_context()?;
    read_sensor_with_context(&mut env, &context, 2)
}

pub fn magnetometer_watch(interval_ms: u32) -> Result<SensorStream<SensorData>, SensorError> {
    if !magnetometer_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(Box::pin(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        match magnetometer_read().await {
            Ok(data) => Some((data, ())),
            _ => None,
        }
    })))
}

pub fn barometer_available() -> bool {
    if let Ok((mut env, context)) = get_env_and_context() {
        is_sensor_available_with_context(&mut env, &context, 6).unwrap_or(false)
    } else {
        false
    }
}

pub async fn barometer_read() -> Result<ScalarData, SensorError> {
    let (mut env, context) = get_env_and_context()?;
    read_pressure_with_context(&mut env, &context)
}

pub fn barometer_watch(interval_ms: u32) -> Result<SensorStream<ScalarData>, SensorError> {
    if !barometer_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(Box::pin(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        match barometer_read().await {
            Ok(data) => Some((data, ())),
            _ => None,
        }
    })))
}

pub fn ambient_light_available() -> bool {
    if let Ok((mut env, context)) = get_env_and_context() {
        is_sensor_available_with_context(&mut env, &context, 5).unwrap_or(false)
    } else {
        false
    }
}

pub async fn ambient_light_read() -> Result<ScalarData, SensorError> {
    let (mut env, context) = get_env_and_context()?;
    read_light_with_context(&mut env, &context)
}

pub fn ambient_light_watch(interval_ms: u32) -> Result<SensorStream<ScalarData>, SensorError> {
    if !ambient_light_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(Box::pin(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        match ambient_light_read().await {
            Ok(data) => Some((data, ())),
            _ => None,
        }
    })))
}
