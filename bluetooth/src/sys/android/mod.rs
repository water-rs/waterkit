#![allow(
    clippy::missing_const_for_fn,
    clippy::unused_self,
    clippy::wildcard_imports
)]

use crate::{
    AdapterState, BluetoothError, ClassicDevice, DeviceId, GattService, ScanFilter, ScanResult,
    Uuid,
};
use jni::JNIEnv;
use jni::objects::{GlobalRef, JObject, JValue};
use std::sync::OnceLock;

static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));
static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();

fn init_dex(env: &mut JNIEnv, context: &JObject) -> Result<(), BluetoothError> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }
    let cache_dir = env
        .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| BluetoothError::PlatformError(format!("getCacheDir: {e}")))?;
    let cache_path = env
        .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| BluetoothError::PlatformError(format!("getAbsolutePath: {e}")))?;
    let dex_path = format!(
        "{}/waterkit_bluetooth.dex",
        env.get_string((&cache_path).into())
            .map_err(|e| BluetoothError::PlatformError(format!("get_string: {e}")))?
            .to_str()
            .map_err(|e| BluetoothError::PlatformError(format!("to_str: {e}")))?
    );
    let _ = std::fs::remove_file(&dex_path);
    std::fs::write(&dex_path, DEX_BYTES)
        .map_err(|e| BluetoothError::PlatformError(format!("write DEX: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dex_path)
            .map_err(|e| BluetoothError::PlatformError(format!("metadata DEX: {e}")))?
            .permissions();
        perms.set_mode(0o444);
        std::fs::set_permissions(&dex_path, perms)
            .map_err(|e| BluetoothError::PlatformError(format!("set_permissions DEX: {e}")))?;
    }
    let dex_path_jstring = env
        .new_string(&dex_path)
        .map_err(|e| BluetoothError::PlatformError(format!("new_string: {e}")))?;
    let parent_loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| BluetoothError::PlatformError(format!("getClassLoader: {e}")))?;
    let dex_class = env
        .find_class("dalvik/system/DexClassLoader")
        .map_err(|e| BluetoothError::PlatformError(format!("find_class: {e}")))?;
    let loader = env
        .new_object(
            dex_class,
            "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/ClassLoader;)V",
            &[
                JValue::Object(&dex_path_jstring),
                JValue::Object(&cache_path),
                JValue::Object(&JObject::null()),
                JValue::Object(&parent_loader),
            ],
        )
        .map_err(|e| BluetoothError::PlatformError(format!("new_object: {e}")))?;
    let global = env
        .new_global_ref(loader)
        .map_err(|e| BluetoothError::PlatformError(format!("global_ref: {e}")))?;
    let _ = CLASS_LOADER.set(global);
    Ok(())
}

#[allow(clippy::unused_async)]
pub async fn adapter_state() -> Result<AdapterState, BluetoothError> {
    Err(BluetoothError::PlatformError(
        "Android: use JNI context directly".into(),
    ))
}

#[derive(Debug)]
pub struct BleScannerInner;

impl BleScannerInner {
    #[allow(clippy::unused_async)]
    pub async fn new() -> Result<Self, BluetoothError> {
        Err(BluetoothError::PlatformError(
            "Android: use JNI context directly".into(),
        ))
    }

    pub fn start_scan(
        &self,
        _filter: &ScanFilter,
    ) -> Result<async_channel::Receiver<ScanResult>, BluetoothError> {
        Err(BluetoothError::PlatformError(
            "Android: use JNI context directly".into(),
        ))
    }

    pub fn stop_scan(&self) {}
}

#[derive(Debug)]
pub struct BleConnectionInner;

impl BleConnectionInner {
    #[allow(clippy::unused_async)]
    pub async fn connect(_device_id: &DeviceId) -> Result<Self, BluetoothError> {
        Err(BluetoothError::PlatformError(
            "Android: use JNI context directly".into(),
        ))
    }

    #[allow(clippy::unused_async)]
    pub async fn discover_services(&self) -> Result<Vec<GattService>, BluetoothError> {
        Err(BluetoothError::NotSupported)
    }

    #[allow(clippy::unused_async)]
    pub async fn read_characteristic(
        &self,
        _service: &Uuid,
        _characteristic: &Uuid,
    ) -> Result<Vec<u8>, BluetoothError> {
        Err(BluetoothError::NotSupported)
    }

    #[allow(clippy::unused_async)]
    pub async fn write_characteristic(
        &self,
        _service: &Uuid,
        _characteristic: &Uuid,
        _data: &[u8],
    ) -> Result<(), BluetoothError> {
        Err(BluetoothError::NotSupported)
    }

    pub fn subscribe(
        &self,
        _service: &Uuid,
        _characteristic: &Uuid,
    ) -> Result<async_channel::Receiver<Vec<u8>>, BluetoothError> {
        Err(BluetoothError::NotSupported)
    }

    #[allow(clippy::unused_async)]
    pub async fn disconnect(self) {}
}

#[derive(Debug)]
pub struct ClassicBluetoothInner;

impl ClassicBluetoothInner {
    #[allow(clippy::unused_async)]
    pub async fn new() -> Result<Self, BluetoothError> {
        Err(BluetoothError::PlatformError(
            "Android: use JNI context directly".into(),
        ))
    }

    pub fn start_discovery(
        &self,
    ) -> Result<async_channel::Receiver<ClassicDevice>, BluetoothError> {
        Err(BluetoothError::NotSupported)
    }

    pub fn stop_discovery(&self) {}

    #[allow(clippy::unused_async)]
    pub async fn paired_devices(&self) -> Result<Vec<ClassicDevice>, BluetoothError> {
        Err(BluetoothError::NotSupported)
    }

    #[allow(clippy::unused_async)]
    pub async fn connect_spp(
        &self,
        _device_id: &DeviceId,
        _uuid: &Uuid,
    ) -> Result<SppStreamInner, BluetoothError> {
        Err(BluetoothError::NotSupported)
    }
}

#[derive(Debug)]
pub struct SppStreamInner;

impl SppStreamInner {
    #[allow(clippy::unused_async)]
    pub async fn read(&self, _buf: &mut [u8]) -> Result<usize, BluetoothError> {
        Err(BluetoothError::NotSupported)
    }

    #[allow(clippy::unused_async)]
    pub async fn write(&self, _data: &[u8]) -> Result<usize, BluetoothError> {
        Err(BluetoothError::NotSupported)
    }

    #[allow(clippy::unused_async)]
    pub async fn close(self) {}
}

/// Android-specific Bluetooth functions requiring JNI context.
pub mod jni_api {
    use super::*;

    /// Get adapter state with JNI context.
    ///
    /// # Errors
    /// Returns error if JNI operations fail.
    pub fn get_adapter_state(
        env: &mut JNIEnv,
        context: &JObject,
    ) -> Result<AdapterState, BluetoothError> {
        init_dex(env, context)?;
        let helper_class_name = env
            .new_string("waterkit.bluetooth.BluetoothHelper")
            .map_err(|e| BluetoothError::PlatformError(format!("new_string: {e}")))?;
        let loader = CLASS_LOADER
            .get()
            .ok_or_else(|| BluetoothError::PlatformError("Class loader not initialized".into()))?;
        let cls = env
            .call_method(
                loader.as_obj(),
                "loadClass",
                "(Ljava/lang/String;)Ljava/lang/Class;",
                &[JValue::Object(&helper_class_name)],
            )
            .and_then(jni::objects::JValueGen::l)
            .map_err(|e| BluetoothError::PlatformError(format!("loadClass: {e}")))?;
        let state = env
            .call_static_method(
                <jni::objects::JObject as Into<jni::objects::JClass>>::into(cls),
                "getAdapterState",
                "(Landroid/content/Context;)I",
                &[JValue::Object(context)],
            )
            .map_err(|e| BluetoothError::PlatformError(format!("getAdapterState: {e}")))?
            .i()
            .map_err(|e| BluetoothError::PlatformError(format!("return: {e}")))?;
        match state {
            0 => Ok(AdapterState::PoweredOff),
            1 => Ok(AdapterState::PoweredOn),
            2 => Ok(AdapterState::Unavailable),
            3 => Ok(AdapterState::Unauthorized),
            _ => Ok(AdapterState::Unknown),
        }
    }
}
