use jni::JNIEnv;
use jni::objects::{GlobalRef, JObject, JValue};
use std::path::PathBuf;
use std::sync::OnceLock;

/// Embedded DEX bytecode containing FsHelper class.
static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

/// Cached class loader for the embedded DEX.
static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();

/// Initialize the DEX class loader. Must be called with a valid Context.
pub fn init_with_context(env: &mut JNIEnv, context: &JObject) -> jni::errors::Result<()> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }

    // Write DEX to cache directory
    let cache_dir = env
        .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])?
        .l()?;

    let cache_path = env
        .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])?
        .l()?;

    let cache_path_str: String = env.get_string((&cache_path).into())?.into();
    let dex_path = format!("{}/waterkit_fs.dex", cache_path_str);

    // Write DEX bytes to file
    std::fs::write(&dex_path, DEX_BYTES).unwrap_or_else(|e| {
        eprintln!("Failed to write DEX: {}", e);
    });

    // Create DexClassLoader
    let dex_path_jstring = env.new_string(&dex_path)?;
    let parent_loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?
        .l()?;

    let dex_class_loader_class = env.find_class("dalvik/system/DexClassLoader")?;
    let class_loader = env.new_object(
        dex_class_loader_class,
        "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/ClassLoader;)V",
        &[
            JValue::Object(&dex_path_jstring),
            JValue::Object(&cache_path),
            JValue::Object(&JObject::null()),
            JValue::Object(&parent_loader),
        ],
    )?;

    let global_ref = env.new_global_ref(class_loader)?;
    let _ = CLASS_LOADER.set(global_ref);
    Ok(())
}

fn call_helper_method(
    env: &mut JNIEnv,
    context: &JObject,
    method_name: &str,
) -> jni::errors::Result<Option<String>> {
    init_with_context(env, context)?;

    let class_loader = CLASS_LOADER.get().unwrap();
    let helper_class_name = env.new_string("waterkit.fs.FsHelper")?;

    let helper_class = env
        .call_method(
            class_loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&helper_class_name)],
        )?
        .l()?;

    let helper_jclass: jni::objects::JClass = helper_class.into();
    let result = env.call_static_method(
        helper_jclass,
        method_name,
        "(Landroid/content/Context;)Ljava/lang/String;",
        &[JValue::Object(context)],
    )?;

    let obj = result.l()?;
    if obj.is_null() {
        Ok(None)
    } else {
        let s: String = env.get_string((&obj).into())?.into();
        Ok(Some(s))
    }
}

pub fn documents_dir_with_context(env: &mut JNIEnv, context: &JObject) -> Option<PathBuf> {
    call_helper_method(env, context, "getDocumentsDir")
        .unwrap_or_else(|e| {
            eprintln!("Error getting documents dir: {}", e);
            None
        })
        .map(PathBuf::from)
}

pub fn cache_dir_with_context(env: &mut JNIEnv, context: &JObject) -> Option<PathBuf> {
    call_helper_method(env, context, "getCacheDir")
        .unwrap_or_else(|e| {
            eprintln!("Error getting cache dir: {}", e);
            None
        })
        .map(PathBuf::from)
}

pub fn documents_dir() -> Option<PathBuf> {
    eprintln!("Android: documents_dir requires Context.");
    None
}

pub fn cache_dir() -> Option<PathBuf> {
    eprintln!("Android: cache_dir requires Context.");
    None
}
