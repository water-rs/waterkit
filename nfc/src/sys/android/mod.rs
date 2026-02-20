use crate::{NdefMessage, NfcError, NfcTag};
use jni::JNIEnv;
use jni::objects::{GlobalRef, JObject, JValue};
use std::sync::OnceLock;

static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));
static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();

fn init_dex(env: &mut JNIEnv, context: &JObject) -> Result<(), NfcError> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }
    let cache_dir = env
        .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| NfcError::PlatformError(format!("getCacheDir: {e}")))?;
    let cache_path = env
        .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| NfcError::PlatformError(format!("getAbsolutePath: {e}")))?;
    let dex_path = format!(
        "{}/waterkit_nfc.dex",
        env.get_string((&cache_path).into())
            .map_err(|e| NfcError::PlatformError(format!("get_string: {e}")))?
            .to_str()
            .map_err(|e| NfcError::PlatformError(format!("to_str: {e}")))?
    );
    let _ = std::fs::remove_file(&dex_path);
    std::fs::write(&dex_path, DEX_BYTES)
        .map_err(|e| NfcError::PlatformError(format!("write DEX: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dex_path)
            .map_err(|e| NfcError::PlatformError(format!("metadata DEX: {e}")))?
            .permissions();
        perms.set_mode(0o444);
        std::fs::set_permissions(&dex_path, perms)
            .map_err(|e| NfcError::PlatformError(format!("set_permissions DEX: {e}")))?;
    }
    let dex_path_jstring = env
        .new_string(&dex_path)
        .map_err(|e| NfcError::PlatformError(format!("new_string: {e}")))?;
    let parent_loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| NfcError::PlatformError(format!("getClassLoader: {e}")))?;
    let dex_class = env
        .find_class("dalvik/system/DexClassLoader")
        .map_err(|e| NfcError::PlatformError(format!("find_class: {e}")))?;
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
        .map_err(|e| NfcError::PlatformError(format!("new_object: {e}")))?;
    let global = env
        .new_global_ref(loader)
        .map_err(|e| NfcError::PlatformError(format!("global_ref: {e}")))?;
    let _ = CLASS_LOADER.set(global);
    Ok(())
}

pub const fn nfc_is_available() -> bool {
    false
}

#[derive(Debug)]
pub struct NfcReaderInner;

impl NfcReaderInner {
    #[allow(clippy::unused_async)]
    pub async fn start_session(
        _message: &str,
    ) -> Result<(Self, async_channel::Receiver<NfcTag>), NfcError> {
        Err(NfcError::PlatformError(
            "Android: use JNI context directly".into(),
        ))
    }

    #[allow(clippy::unused_async)]
    pub async fn write(&self, _message: NdefMessage) -> Result<(), NfcError> {
        Err(NfcError::NotSupported)
    }

    pub const fn stop(&self) {
        let _ = self;
    }
}

/// Android-specific NFC functions requiring JNI context.
pub mod jni_api {
    use super::{CLASS_LOADER, JNIEnv, JObject, JValue, NfcError, init_dex};

    /// Check if NFC is available with JNI context.
    ///
    /// # Errors
    /// Returns error if JNI operations fail.
    pub fn is_available(env: &mut JNIEnv, context: &JObject) -> Result<bool, NfcError> {
        init_dex(env, context)?;
        let helper_class_name = env
            .new_string("waterkit.nfc.NfcHelper")
            .map_err(|e| NfcError::PlatformError(format!("new_string: {e}")))?;
        let loader = CLASS_LOADER
            .get()
            .ok_or_else(|| NfcError::PlatformError("Class loader not initialized".into()))?;
        let cls = env
            .call_method(
                loader.as_obj(),
                "loadClass",
                "(Ljava/lang/String;)Ljava/lang/Class;",
                &[JValue::Object(&helper_class_name)],
            )
            .and_then(jni::objects::JValueGen::l)
            .map_err(|e| NfcError::PlatformError(format!("loadClass: {e}")))?;
        let result = env
            .call_static_method(
                <jni::objects::JObject as Into<jni::objects::JClass>>::into(cls),
                "isAvailable",
                "(Landroid/content/Context;)Z",
                &[JValue::Object(context)],
            )
            .map_err(|e| NfcError::PlatformError(format!("isAvailable: {e}")))?
            .z()
            .map_err(|e| NfcError::PlatformError(format!("return: {e}")))?;
        Ok(result)
    }
}
