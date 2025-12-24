use crate::{Error, ScreenInfo};
use jni::objects::{GlobalRef, JObject, JValue};
use jni::{JNIEnv, JavaVM};
use std::sync::OnceLock;

static GLOBAL_CONTEXT: OnceLock<GlobalRef> = OnceLock::new();
static JAVA_VM: OnceLock<JavaVM> = OnceLock::new();

/// Initialize the screen subsystem with a Context.
/// This must be called before using any screen APIs on Android.
pub fn init(env: &mut JNIEnv, context: &JObject) -> Result<(), Error> {
    if GLOBAL_CONTEXT.get().is_some() {
        return Ok(());
    }

    if JAVA_VM.get().is_none() {
        let vm = env.get_java_vm()
            .map_err(|e| Error::Platform(format!("get_java_vm failed: {e}")))?;
        let _ = JAVA_VM.set(vm);
    }

    let context_ref = env.new_global_ref(context)
        .map_err(|e| Error::Platform(format!("new_global_ref context failed: {e}")))?;
    let _ = GLOBAL_CONTEXT.set(context_ref);

    Ok(())
}

fn get_env_and_context() -> Result<(jni::AttachGuard<'static>, JObject<'static>), Error> {
    let vm = JAVA_VM.get()
        .ok_or_else(|| Error::Platform("JavaVM not initialized. Call init() first.".into()))?;
    let context_ref = GLOBAL_CONTEXT.get()
        .ok_or_else(|| Error::Platform("Context not initialized. Call init() first.".into()))?;
    
    let env = vm.attach_current_thread()
        .map_err(|e| Error::Platform(format!("attach_current_thread failed: {e}")))?;
    
    let context = context_ref.as_obj();
    let local_ref = env.new_local_ref(context)
        .map_err(|e| Error::Platform(format!("new_local_ref failed: {e}")))?;
    Ok((env, local_ref))
}


pub fn capture_screen(display_index: usize) -> Result<Vec<u8>, Error> {
    // TODO: Implement using MediaProjection or View snapshotting.
    // This is complex and requires Activity/Permission.
    // For now returning Unsupported.
    Err(Error::Unsupported)
}

pub fn screens() -> Result<Vec<ScreenInfo>, Error> {
    // TODO: Implement DisplayManager query
    // Minimal placeholder
    Ok(vec![ScreenInfo {
        id: 0,
        name: "Android Display".into(),
        width: 0,
        height: 0,
        scale_factor: 1.0,
        is_primary: true
    }])
}

pub async fn get_brightness() -> Result<f32, Error> {
    // Settings.System.getInt(contentResolver, SCREEN_BRIGHTNESS)
    let (mut env, context) = get_env_and_context()?;
    
    let content_resolver = env.call_method(&context, "getContentResolver", "()Landroid/content/ContentResolver;", &[])
        .map_err(|e| Error::Platform(e.to_string()))?.l()
        .map_err(|e| Error::Platform(e.to_string()))?;
        
    let settings_system_class = env.find_class("android/provider/Settings$System")
        .map_err(|e| Error::Platform(e.to_string()))?;
        
    let name = env.new_string("screen_brightness").map_err(|e| Error::Platform(e.to_string()))?;
    
    let val = env.call_static_method(
        settings_system_class, 
        "getInt", 
        "(Landroid/content/ContentResolver;Ljava/lang/String;)I", 
        &[JValue::Object(&content_resolver), JValue::Object(&name)]
    ).map_err(|e| Error::Platform(e.to_string()))?.i().map_err(|e| Error::Platform(e.to_string()))?;
    
    // 0-255 usually
    Ok(val as f32 / 255.0)
}

pub async fn set_brightness(val: f32) -> Result<(), Error> {
    // Settings.System.putInt(contentResolver, SCREEN_BRIGHTNESS, val)
    let (mut env, context) = get_env_and_context()?;
    
    let content_resolver = env.call_method(&context, "getContentResolver", "()Landroid/content/ContentResolver;", &[])
        .map_err(|e| Error::Platform(e.to_string()))?.l()
        .map_err(|e| Error::Platform(e.to_string()))?;
        
    let settings_system_class = env.find_class("android/provider/Settings$System")
        .map_err(|e| Error::Platform(e.to_string()))?;
        
    let name = env.new_string("screen_brightness").map_err(|e| Error::Platform(e.to_string()))?;
    let int_val = (val.clamp(0.0, 1.0) * 255.0) as i32;
    
    let _ = env.call_static_method(
        settings_system_class,
        "putInt",
        "(Landroid/content/ContentResolver;Ljava/lang/String;I)Z",
        &[JValue::Object(&content_resolver), JValue::Object(&name), JValue::Int(int_val)]
    ).map_err(|e| Error::Platform(e.to_string()))?;
    
    Ok(())
}

pub async fn pick_and_capture() -> Result<Vec<u8>, Error> {
    Err(Error::Unsupported)
}
