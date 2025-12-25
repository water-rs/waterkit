//! Android camera implementation using Camera2 API via JNI.

use crate::{CameraError, CameraFrame, CameraInfo, FrameFormat, Resolution};
use jni::JNIEnv;
use jni::objects::{GlobalRef, JObject, JString, JValue};
use std::sync::{Arc, Mutex, OnceLock};

/// Embedded DEX bytecode containing CameraHelper class.
/// Generated at build time by kotlinc + D8.
static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

/// Cached class loader for the embedded DEX.
static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();

/// Cached context reference.
static CONTEXT: OnceLock<GlobalRef> = OnceLock::new();

/// Initialize the DEX class loader with a valid Android Context.
///
/// # Safety
/// The `context` must be a valid Android Context JObject.
pub fn init_with_context(env: &mut JNIEnv, context: &JObject) -> Result<(), CameraError> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }

    // Store context for later use
    let context_ref = env
        .new_global_ref(context)
        .map_err(|e| CameraError::OpenFailed(format!("new_global_ref context: {e}")))?;
    let _ = CONTEXT.set(context_ref);

    // Write DEX to cache directory
    let cache_dir = env
        .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])
        .map_err(|e| CameraError::OpenFailed(format!("getCacheDir: {e}")))?
        .l()
        .map_err(|e| CameraError::OpenFailed(format!("getCacheDir result: {e}")))?;

    let cache_path = env
        .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .map_err(|e| CameraError::OpenFailed(format!("getAbsolutePath: {e}")))?
        .l()
        .map_err(|e| CameraError::OpenFailed(format!("getAbsolutePath result: {e}")))?;

    let dex_path = format!(
        "{}/waterkit_camera.dex",
        env.get_string((&cache_path).into())
            .map_err(|e| CameraError::OpenFailed(format!("get_string: {e}")))?
            .to_str()
            .map_err(|e| CameraError::OpenFailed(format!("to_str: {e}")))?
    );

    // Write DEX bytes to file
    std::fs::write(&dex_path, DEX_BYTES)
        .map_err(|e| CameraError::OpenFailed(format!("write DEX: {e}")))?;

    // Create DexClassLoader
    let dex_path_jstring = env
        .new_string(&dex_path)
        .map_err(|e| CameraError::OpenFailed(format!("new_string: {e}")))?;

    let parent_loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .map_err(|e| CameraError::OpenFailed(format!("getClassLoader: {e}")))?
        .l()
        .map_err(|e| CameraError::OpenFailed(format!("getClassLoader result: {e}")))?;

    let dex_class_loader_class = env
        .find_class("dalvik/system/DexClassLoader")
        .map_err(|e| CameraError::OpenFailed(format!("find DexClassLoader: {e}")))?;

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
        .map_err(|e| CameraError::OpenFailed(format!("new DexClassLoader: {e}")))?;

    let global_ref = env
        .new_global_ref(class_loader)
        .map_err(|e| CameraError::OpenFailed(format!("new_global_ref: {e}")))?;

    let _ = CLASS_LOADER.set(global_ref);
    Ok(())
}

/// Get the CameraHelper class.
fn get_helper_class<'a>(env: &mut JNIEnv<'a>) -> Result<JObject<'a>, CameraError> {
    let class_loader = CLASS_LOADER
        .get()
        .ok_or_else(|| CameraError::OpenFailed("Class loader not initialized".into()))?;

    let helper_class_name = env
        .new_string("waterkit.camera.CameraHelper")
        .map_err(|e| CameraError::Unknown(format!("new_string: {e}")))?;

    let helper_class = env
        .call_method(
            class_loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&helper_class_name)],
        )
        .map_err(|e| CameraError::Unknown(format!("loadClass: {e}")))?
        .l()
        .map_err(|e| CameraError::Unknown(format!("loadClass result: {e}")))?;

    Ok(helper_class)
}

/// List cameras using the Kotlin helper.
pub fn list_cameras_with_context(env: &mut JNIEnv) -> Result<Vec<CameraInfo>, CameraError> {
    let helper_class = get_helper_class(env)?;
    let context = CONTEXT
        .get()
        .ok_or_else(|| CameraError::OpenFailed("Context not initialized".into()))?;

    let result = env
        .call_static_method(
            (&helper_class).into(),
            "listCameras",
            "(Landroid/content/Context;)[[Ljava/lang/String;",
            &[JValue::Object(context.as_obj())],
        )
        .map_err(|e| CameraError::EnumerationFailed(format!("listCameras: {e}")))?
        .l()
        .map_err(|e| CameraError::EnumerationFailed(format!("listCameras result: {e}")))?;

    // Parse the 2D string array
    let array = unsafe { jni::objects::JObjectArray::from_raw(result.into_raw()) };
    let length = env.get_array_length(&array).unwrap_or(0);

    let mut cameras = Vec::new();
    for i in 0..length {
        let inner = env.get_object_array_element(&array, i).ok();
        if let Some(inner) = inner {
            let inner_array = unsafe { jni::objects::JObjectArray::from_raw(inner.into_raw()) };
            let id: JString = env
                .get_object_array_element(&inner_array, 0)
                .ok()
                .map(|o| o.into())
                .unwrap_or_default();
            let name: JString = env
                .get_object_array_element(&inner_array, 1)
                .ok()
                .map(|o| o.into())
                .unwrap_or_default();
            let is_front: JString = env
                .get_object_array_element(&inner_array, 2)
                .ok()
                .map(|o| o.into())
                .unwrap_or_default();

            let id_str = env.get_string(&id).map(|s| s.into()).unwrap_or_default();
            let name_str = env.get_string(&name).map(|s| s.into()).unwrap_or_default();
            let is_front_str: String = env
                .get_string(&is_front)
                .map(|s| s.into())
                .unwrap_or_default();

            cameras.push(CameraInfo {
                id: id_str,
                name: name_str,
                description: None,
                is_front_facing: is_front_str == "true",
            });
        }
    }

    Ok(cameras)
}

// CameraInner implementation using JNI
#[derive(Debug)]
pub struct CameraInner {
    resolution: Arc<Mutex<Resolution>>,
    camera_id: String,
}

impl CameraInner {
    pub fn list() -> Result<Vec<CameraInfo>, CameraError> {
        // This should not be called theoretically as the public API calls list_cameras_with_context
        // But if it is, we can try to use the cached context
        let mut env = unsafe {
            let vm = jni::JavaVM::from_raw(ndk_context::android_context().vm().cast())
                .map_err(|e| CameraError::Unknown(format!("vm attach: {e}")))?;
            vm.attach_current_thread()
                .map_err(|e| CameraError::Unknown(format!("env attach: {e}")))?
        };
        list_cameras_with_context(&mut env)
    }

    pub fn open(camera_id: &str) -> Result<Self, CameraError> {
        // Get generic environment
        let mut env = unsafe {
            let vm = jni::JavaVM::from_raw(ndk_context::android_context().vm().cast())
                .map_err(|e| CameraError::Unknown(format!("vm attach: {e}")))?;
            vm.attach_current_thread()
                .map_err(|e| CameraError::Unknown(format!("env attach: {e}")))?
        };

        let helper_class = get_helper_class(&mut env)?;
        let context = CONTEXT
            .get()
            .ok_or_else(|| CameraError::OpenFailed("Context not initialized".into()))?;

        let id_jstr = env
            .new_string(camera_id)
            .map_err(|e| CameraError::OpenFailed(format!("new_string: {e}")))?;

        let result = env
            .call_static_method(
                (&helper_class).into(),
                "openCamera",
                "(Landroid/content/Context;Ljava/lang/String;)Z",
                &[JValue::Object(context.as_obj()), JValue::Object(&id_jstr)],
            )
            .map_err(|e| CameraError::OpenFailed(format!("openCamera: {e}")))?
            .z()
            .map_err(|e| CameraError::OpenFailed(format!("openCamera result: {e}")))?;

        if !result {
            return Err(CameraError::OpenFailed(format!(
                "Failed to open camera {camera_id}"
            )));
        }

        Ok(Self {
            resolution: Arc::new(Mutex::new(Resolution::HD)),
            camera_id: camera_id.to_string(),
        })
    }

    pub fn start(&mut self) -> Result<(), CameraError> {
        let mut env = unsafe {
            let vm = jni::JavaVM::from_raw(ndk_context::android_context().vm().cast())
                .map_err(|e| CameraError::Unknown(format!("vm attach: {e}")))?;
            vm.attach_current_thread()
                .map_err(|e| CameraError::Unknown(format!("env attach: {e}")))?
        };

        let helper_class = get_helper_class(&mut env)?;

        let result = env
            .call_static_method((&helper_class).into(), "startCapture", "()Z", &[])
            .map_err(|e| CameraError::StartFailed(format!("startCapture: {e}")))?
            .z()
            .map_err(|e| CameraError::StartFailed(format!("startCapture result: {e}")))?;

        if !result {
            return Err(CameraError::StartFailed("Failed to start capture".into()));
        }
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), CameraError> {
        let mut env = unsafe {
            let vm = jni::JavaVM::from_raw(ndk_context::android_context().vm().cast())
                .map_err(|e| CameraError::Unknown(format!("vm attach: {e}")))?;
            vm.attach_current_thread()
                .map_err(|e| CameraError::Unknown(format!("env attach: {e}")))?
        };

        let helper_class = get_helper_class(&mut env)?;

        env.call_static_method((&helper_class).into(), "stopCapture", "()V", &[])
            .map_err(|e| CameraError::Unknown(format!("stopCapture: {e}")))?;

        Ok(())
    }

    pub fn get_frame(&mut self) -> Result<CameraFrame, CameraError> {
        let mut env = unsafe {
            let vm = jni::JavaVM::from_raw(ndk_context::android_context().vm().cast())
                .map_err(|e| CameraError::Unknown(format!("vm attach: {e}")))?;
            vm.attach_current_thread()
                .map_err(|e| CameraError::Unknown(format!("env attach: {e}")))?
        };

        let helper_class = get_helper_class(&mut env)?;

        let result = env
            .call_static_method((&helper_class).into(), "getFrame", "()[B", &[])
            .map_err(|e| CameraError::CaptureFailed(format!("getFrame: {e}")))?
            .l()
            .map_err(|e| CameraError::CaptureFailed(format!("getFrame result: {e}")))?;

        if result.is_null() {
             // Non-blocking return if no frame, or block? API says "may block".
             // For now, if null, we can sleep a bit or return an error/empty.
             // But CameraHelper uses latestFrame which is reset to null.
             // We should loop or implement blocking in Kotlin.
             // For simplicity, let's retry a few times or return NotReady/error.
             // The trait implies blocking is allowed.
             std::thread::sleep(std::time::Duration::from_millis(16));
             return self.get_frame(); // Simple recursion for blocking
        }

        let array: jni::objects::JByteArray = result.into();
        let bytes = env
            .convert_byte_array(&array)
            .map_err(|e| CameraError::CaptureFailed(format!("convert byte array: {e}")))?;

        // Get size
        let size_result = env
            .call_static_method((&helper_class).into(), "getFrameSize", "()[I", &[])
            .map_err(|e| CameraError::CaptureFailed(format!("getFrameSize: {e}")))?
            .l()
            .map_err(|e| CameraError::CaptureFailed(format!("getFrameSize result: {e}")))?;
        
        let size_array: jni::objects::JIntArray = size_result.into();
        let mut sizes = [0i32; 2];
        env.get_int_array_region(&size_array, 0, &mut sizes)
            .map_err(|e| CameraError::CaptureFailed(format!("get_int_array_region: {e}")))?;

        let width = sizes[0] as u32;
        let height = sizes[1] as u32;

        Ok(CameraFrame {
            data: bytes,
            width,
            height,
            format: FrameFormat::Rgba, // Kotlin converts to RGBA
            native_handle: None,
        })
    }

    pub fn set_resolution(&mut self, resolution: Resolution) -> Result<(), CameraError> {
        // TODO: Update Kotlin side resolution
        let mut lock = self.resolution.lock().unwrap();
        *lock = resolution;
        Ok(())
    }

    pub fn resolution(&self) -> Resolution {
        *self.resolution.lock().unwrap()
    }

    pub fn dropped_frame_count(&self) -> u64 {
        0
    }

    pub fn set_hdr(&self, _enabled: bool) -> Result<(), CameraError> {
        Err(CameraError::NotSupported)
    }

    pub fn hdr_enabled(&self) -> bool {
        false
    }

    pub fn take_photo(&mut self) -> Result<CameraFrame, CameraError> {
        self.get_frame() // Just take next frame for now
    }

    pub fn start_recording(&mut self, _path: &str) -> Result<(), CameraError> {
        Err(CameraError::NotSupported)
    }

    pub fn stop_recording(&mut self) -> Result<(), CameraError> {
        Err(CameraError::NotSupported)
    }
}
