use crate::ImageData;
use jni::JNIEnv;
use jni::objects::{GlobalRef, JObject, JValue};
use std::borrow::Cow;
use std::sync::OnceLock;

static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));
static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();

pub fn init_with_context(env: &mut JNIEnv, context: &JObject) -> Result<(), String> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }

    // Standard DEX loading boilerplate
    let cache_dir = env
        .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])
        .and_then(|v| v.l())
        .map_err(|e| format!("JNI error getCacheDir: {e}"))?;

    let cache_path = env
        .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .and_then(|v| v.l())
        .map_err(|e| format!("JNI error getAbsolutePath: {e}"))?;

    let dex_path = format!(
        "{}/waterkit_clipboard.dex",
        env.get_string((&cache_path).into())
            .map_err(|e| format!("JNI error get_string: {e}"))?
            .to_str()
            .map_err(|e| format!("JNI error to_str: {e}"))?
    );

    std::fs::write(&dex_path, DEX_BYTES).map_err(|e| format!("Failed to write DEX: {e}"))?;

    let dex_path_jstring = env
        .new_string(&dex_path)
        .map_err(|e| format!("JNI error new_string: {e}"))?;

    let parent_loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .and_then(|v| v.l())
        .map_err(|e| format!("JNI error getClassLoader: {e}"))?;

    let dex_class_loader_class = env
        .find_class("dalvik/system/DexClassLoader")
        .map_err(|e| format!("JNI error find_class: {e}"))?;

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
        .map_err(|e| format!("JNI error new_object: {e}"))?;

    let global_ref = env
        .new_global_ref(class_loader)
        .map_err(|e| format!("JNI error new_global_ref: {e}"))?;

    let _ = CLASS_LOADER.set(global_ref);
    Ok(())
}

fn get_helper_class<'a>(env: &mut JNIEnv<'a>) -> Result<jni::objects::JClass<'a>, String> {
    let class_loader = CLASS_LOADER
        .get()
        .ok_or_else(|| "Class loader not initialized".to_string())?;

    let helper_class_name = env
        .new_string("waterkit/clipboard/ClipboardHelper")
        .map_err(|e| format!("JNI error new_string name: {e}"))?;

    let helper_class = env
        .call_method(
            class_loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&helper_class_name)],
        )
        .and_then(|v| v.l())
        .map_err(|e| format!("JNI error loadClass: {e}"))?;

    Ok(helper_class.into())
}

pub fn get_text_with_context(
    env: &mut JNIEnv,
    context: &JObject,
) -> Result<Option<String>, String> {
    init_with_context(env, context)?;
    let helper_class = get_helper_class(env)?;

    let result = env
        .call_static_method(
            helper_class,
            "getText",
            "(Landroid/content/Context;)Ljava/lang/String;",
            &[JValue::Object(context)],
        )
        .map_err(|e| format!("JNI error getText: {e}"))?;

    let obj = result.l().map_err(|e| format!("JNI error result: {e}"))?;
    if obj.is_null() {
        Ok(None)
    } else {
        let text = env
            .get_string(obj.into())
            .map_err(|e| format!("JNI error get_string: {e}"))?;
        Ok(Some(text.into()))
    }
}

pub fn set_text_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    text: String,
) -> Result<(), String> {
    init_with_context(env, context)?;
    let helper_class = get_helper_class(env)?;

    let jtext = env
        .new_string(text)
        .map_err(|e| format!("JNI error new_string: {e}"))?;

    env.call_static_method(
        helper_class,
        "setText",
        "(Landroid/content/Context;Ljava/lang/String;)V",
        &[JValue::Object(context), JValue::Object(&jtext)],
    )
    .map_err(|e| format!("JNI error setText: {e}"))?;

    Ok(())
}

pub fn get_image_with_context(
    env: &mut JNIEnv,
    context: &JObject,
) -> Result<Option<ImageData>, String> {
    init_with_context(env, context)?;
    let helper_class = get_helper_class(env)?;

    let result = env
        .call_static_method(
            helper_class,
            "getImage",
            "(Landroid/content/Context;)[B",
            &[JValue::Object(context)],
        )
        .map_err(|e| format!("JNI error getImage: {e}"))?;

    let obj = result.l().map_err(|e| format!("JNI error result: {e}"))?;
    if obj.is_null() {
        Ok(None)
    } else {
        let auto_array = env
            .convert_byte_array(obj.into())
            .map_err(|e| format!("JNI error convert_byte_array: {e}"))?;
        // We don't get width/height from the bytes easily without decoding.
        // arboard expects width/height.
        // We might need to decode the image in Types (on Java side or Rust side).
        // Since we return ImageData with w/h, we should probably decode it.
        // Or we can return raw bytes and let the user handle it?
        // But ImageData struct has width/height.
        // For now, let's return a dummy ImageData with 0x0 and raw bytes, or change ImageData definition?
        // ImageData usually implies raw pixels (RGBA).
        // If arboard expects raw RGBA, then sending valid PNG/JPG bytes is wrong.
        // arboard docs say: "The image data is expected to be in RGBA8888 format."
        // So I need to decode the image on Android side to RGBA and get width/height.
        // Android Bitmap can do this.
        Err("Image decoding not yet implemented on Android bridge".into())
    }
}

pub fn set_image_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    _image: ImageData,
) -> Result<(), String> {
    Err("set_image not implemented on Android".into())
}

// Public API stubs
pub fn get_text() -> Option<String> {
    eprintln!("Android: use get_text_with_context");
    None
}

pub fn set_text(_text: String) {
    eprintln!("Android: use set_text_with_context");
}

pub fn get_image() -> Option<ImageData> {
    eprintln!("Android: use get_image_with_context");
    None
}

pub fn set_image(_image: ImageData) {
    eprintln!("Android: use set_image_with_context");
}
