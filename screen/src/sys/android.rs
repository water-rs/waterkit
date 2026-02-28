//! Android platform implementation using `MediaProjection` API.
//!
//! Screen capture on Android requires:
//!
//! 1. Call `init()` with the application context
//! 2. Request permission via Activity (call `ScreenHelper.getPermissionIntent()` from Kotlin)
//! 3. Pass the result to `onPermissionResult()`
//! 4. Call `startCapture()` to begin screen capture

#![allow(clippy::cast_sign_loss)] // JNI array lengths
#![allow(clippy::cast_possible_truncation)] // Timestamps
#![allow(clippy::similar_names)] // JNI variable naming patterns
#![allow(clippy::option_if_let_else)] // Pattern used for readability
#![allow(clippy::collapsible_if)] // Readability
#![allow(clippy::needless_pass_by_value)] // API design

use crate::frame::ScreenFrame;
use crate::screenshot::{ImageFormat, Screenshot};
use crate::stream::StreamConfig;
use crate::{Error, ScreenInfo};
use jni::JNIEnv;
use jni::objects::{GlobalRef, JClass, JObject, JValue};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use wgpu::{Device, Queue};

/// Embedded DEX bytecode containing `ScreenHelper` class.
/// Generated at build time by kotlinc + D8.
static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

/// Cached class loader for the embedded DEX.
static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();

/// Cached application context.
static CONTEXT: OnceLock<GlobalRef> = OnceLock::new();

fn get_vm_and_context() -> (jni::JavaVM, JObject<'static>) {
    let android_ctx = ndk_context::android_context();
    let vm = unsafe { jni::JavaVM::from_raw(android_ctx.vm().cast()).unwrap() };
    let context = unsafe { JObject::from_raw(android_ctx.context().cast()) };
    (vm, context)
}

fn ensure_dex_loaded() -> Result<(), Error> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }

    let (vm, context) = get_vm_and_context();
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| Error::Platform(format!("attach_current_thread: {e}")))?;

    init_with_context(&mut env, &context)
}

fn init_with_context(env: &mut JNIEnv, context: &JObject) -> Result<(), Error> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }

    // Write DEX to cache directory
    let cache_dir = env
        .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])
        .map_err(|e| Error::Platform(format!("getCacheDir: {e}")))?
        .l()
        .map_err(|e| Error::Platform(format!("getCacheDir result: {e}")))?;

    let cache_path = env
        .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .map_err(|e| Error::Platform(format!("getAbsolutePath: {e}")))?
        .l()
        .map_err(|e| Error::Platform(format!("getAbsolutePath result: {e}")))?;

    let dex_path = format!(
        "{}/waterkit_screen.dex",
        env.get_string((&cache_path).into())
            .map_err(|e| Error::Platform(format!("get_string: {e}")))?
            .to_str()
            .map_err(|e| Error::Platform(format!("to_str: {e}")))?
    );

    // Write DEX bytes to file
    std::fs::write(&dex_path, DEX_BYTES).map_err(|e| Error::Platform(format!("write DEX: {e}")))?;

    // Create DexClassLoader
    let dex_path_jstring = env
        .new_string(&dex_path)
        .map_err(|e| Error::Platform(format!("new_string: {e}")))?;

    let parent_loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .map_err(|e| Error::Platform(format!("getClassLoader: {e}")))?
        .l()
        .map_err(|e| Error::Platform(format!("getClassLoader result: {e}")))?;

    let dex_class_loader_class = env
        .find_class("dalvik/system/DexClassLoader")
        .map_err(|e| Error::Platform(format!("find DexClassLoader: {e}")))?;

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
        .map_err(|e| Error::Platform(format!("new DexClassLoader: {e}")))?;

    let global_ref = env
        .new_global_ref(class_loader)
        .map_err(|e| Error::Platform(format!("new_global_ref: {e}")))?;

    let _ = CLASS_LOADER.set(global_ref);

    // Also initialize the Kotlin helper with context
    let helper_class = get_helper_class(env)?;
    env.call_static_method(
        &helper_class,
        "initWithContext",
        "(Landroid/content/Context;)Z",
        &[JValue::Object(context)],
    )
    .map_err(|e| Error::Platform(format!("initWithContext: {e}")))?;

    Ok(())
}

/// Get the `ScreenHelper` class.
fn get_helper_class<'a>(env: &mut JNIEnv<'a>) -> Result<JClass<'a>, Error> {
    let loader = CLASS_LOADER
        .get()
        .ok_or_else(|| Error::Platform("Class loader not initialized".into()))?;

    let class_name = env
        .new_string("waterkit.screen.ScreenHelper")
        .map_err(|e| Error::Platform(format!("new_string: {e}")))?;

    let loaded = env
        .call_method(
            loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&class_name)],
        )
        .map_err(|e| Error::Platform(format!("loadClass: {e}")))?
        .l()
        .map_err(|e| Error::Platform(format!("loadClass result: {e}")))?;

    Ok(loaded.into())
}

/// Initialize the screen module with Android context.
pub fn init(env: &mut JNIEnv, context: &JObject) -> Result<(), Error> {
    let global = env
        .new_global_ref(context)
        .map_err(|e| Error::Platform(e.to_string()))?;
    let _ = CONTEXT.set(global);
    init_with_context(env, context)
}

/// Enumerate screens (returns single main screen with actual dimensions).
pub fn screens() -> Result<Vec<ScreenInfo>, Error> {
    ensure_dex_loaded()?;

    let (vm, _context) = get_vm_and_context();
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| Error::Platform(format!("env attach: {e}")))?;

    let helper_class = get_helper_class(&mut env)?;

    // Get screen dimensions from helper
    let dims = env
        .call_static_method(&helper_class, "getFrameDimensions", "()[I", &[])
        .map_err(|e| Error::Platform(format!("getFrameDimensions: {e}")))?
        .l()
        .map_err(|e| Error::Platform(format!("getFrameDimensions result: {e}")))?;

    let dims_array: jni::objects::JIntArray = dims.into();
    let mut dims_buf = [0i32; 2];
    env.get_int_array_region(&dims_array, 0, &mut dims_buf)
        .map_err(|e| Error::Platform(format!("get_int_array_region: {e}")))?;

    let width = dims_buf[0].max(1920) as u32;
    let height = dims_buf[1].max(1080) as u32;

    Ok(vec![ScreenInfo::new(
        0,
        "Main Screen".into(),
        width,
        height,
        1.0,
        true,
    )])
}

/// Return the maximum refresh rate reported by Android display metadata.
pub fn max_refresh_rate_hz() -> Result<f32, Error> {
    ensure_dex_loaded()?;

    let (vm, _context) = get_vm_and_context();
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| Error::Platform(format!("env attach: {e}")))?;

    let helper_class = get_helper_class(&mut env)?;
    let refresh_hz = env
        .call_static_method(&helper_class, "getRefreshRateHz", "()F", &[])
        .map_err(|e| Error::Platform(format!("getRefreshRateHz: {e}")))?
        .f()
        .map_err(|e| Error::Platform(format!("getRefreshRateHz result: {e}")))?;

    if refresh_hz.is_finite() && refresh_hz > 0.0 {
        Ok(refresh_hz)
    } else {
        Err(Error::Platform(format!(
            "invalid Android refresh rate value: {refresh_hz}"
        )))
    }
}

/// Capture a screenshot on Android using MediaProjection.
pub fn screenshot(display: &ScreenInfo, format: ImageFormat) -> Result<Screenshot, Error> {
    if !matches!(format, ImageFormat::Png) {
        return Err(Error::Unsupported);
    }

    ensure_dex_loaded()?;

    let (vm, _context) = get_vm_and_context();
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| Error::Platform(format!("env attach: {e}")))?;

    let helper_class = get_helper_class(&mut env)?;
    let has_permission = env
        .call_static_method(&helper_class, "hasPermission", "()Z", &[])
        .map_err(|e| Error::Platform(format!("hasPermission: {e}")))?
        .z()
        .map_err(|e| Error::Platform(format!("hasPermission result: {e}")))?;

    if !has_permission {
        return Err(Error::PermissionDenied);
    }

    let screenshot_obj = env
        .call_static_method(&helper_class, "captureScreenshotPng", "()[B", &[])
        .map_err(|e| Error::Platform(format!("captureScreenshotPng: {e}")))?
        .l()
        .map_err(|e| Error::Platform(format!("captureScreenshotPng result: {e}")))?;

    if screenshot_obj.is_null() {
        return Err(Error::Platform(
            "captureScreenshotPng returned null frame data".into(),
        ));
    }

    let bytes_array: jni::objects::JByteArray = screenshot_obj.into();
    let data = env
        .convert_byte_array(&bytes_array)
        .map_err(|e| Error::Platform(format!("convert_byte_array: {e}")))?;

    if data.is_empty() {
        return Err(Error::Platform(
            "captureScreenshotPng returned empty data".into(),
        ));
    }

    Ok(Screenshot::new(
        data,
        display.width(),
        display.height(),
        format,
    ))
}

/// Get screen brightness.
#[allow(clippy::unused_async)]
pub async fn get_brightness() -> Result<f32, Error> {
    ensure_dex_loaded()?;

    let (vm, _context) = get_vm_and_context();
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| Error::Platform(format!("env attach: {e}")))?;

    let helper_class = get_helper_class(&mut env)?;

    let brightness = env
        .call_static_method(&helper_class, "getBrightness", "()F", &[])
        .map_err(|e| Error::Platform(format!("getBrightness: {e}")))?
        .f()
        .map_err(|e| Error::Platform(format!("getBrightness result: {e}")))?;

    Ok(brightness)
}

/// Set screen brightness.
#[allow(clippy::unused_async)]
pub async fn set_brightness(val: f32) -> Result<(), Error> {
    ensure_dex_loaded()?;

    let (vm, _context) = get_vm_and_context();
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| Error::Platform(format!("env attach: {e}")))?;

    let helper_class = get_helper_class(&mut env)?;

    let result = env
        .call_static_method(
            &helper_class,
            "setBrightness",
            "(F)Z",
            &[JValue::Float(val)],
        )
        .map_err(|e| Error::Platform(format!("setBrightness: {e}")))?
        .z()
        .map_err(|e| Error::Platform(format!("setBrightness result: {e}")))?;

    if result {
        Ok(())
    } else {
        Err(Error::Platform("Failed to set brightness".into()))
    }
}

/// Raw frame data from Android `MediaProjection`.
struct RawFrame {
    data: Vec<u8>,
    width: u32,
    height: u32,
    timestamp_ns: u64,
}

/// Screen stream using `MediaProjection`.
pub struct ScreenStreamInner {
    device: Arc<Device>,
    queue: Arc<Queue>,
    width: u32,
    height: u32,
    running: Arc<AtomicBool>,
    frame_receiver: async_channel::Receiver<RawFrame>,
}

impl ScreenStreamInner {
    pub fn new(
        display: &ScreenInfo,
        device: Arc<Device>,
        queue: Arc<Queue>,
        _config: &StreamConfig,
    ) -> Result<Self, Error> {
        ensure_dex_loaded()?;

        let (vm, _context) = get_vm_and_context();
        let mut env = vm
            .attach_current_thread()
            .map_err(|e| Error::Platform(format!("env attach: {e}")))?;

        let helper_class = get_helper_class(&mut env)?;

        // Check if we have permission
        let has_permission = env
            .call_static_method(&helper_class, "hasPermission", "()Z", &[])
            .map_err(|e| Error::Platform(format!("hasPermission: {e}")))?
            .z()
            .map_err(|e| Error::Platform(format!("hasPermission result: {e}")))?;

        if !has_permission {
            return Err(Error::PermissionDenied);
        }

        // Start capture
        let started = env
            .call_static_method(&helper_class, "startCapture", "()Z", &[])
            .map_err(|e| Error::Platform(format!("startCapture: {e}")))?
            .z()
            .map_err(|e| Error::Platform(format!("startCapture result: {e}")))?;

        if !started {
            return Err(Error::Platform("Failed to start screen capture".into()));
        }

        let (sender, receiver) = async_channel::bounded(2);
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();
        let width = display.width();
        let height = display.height();

        // Spawn frame capture thread
        std::thread::spawn(move || {
            let (vm, _ctx) = get_vm_and_context();
            let Ok(mut env) = vm.attach_current_thread() else {
                return;
            };

            while running_clone.load(Ordering::SeqCst) {
                let Ok(helper_class) = get_helper_class(&mut env) else {
                    break;
                };

                let frame_result = env
                    .call_static_method(&helper_class, "getFrame", "()[B", &[])
                    .ok()
                    .and_then(|r| r.l().ok());

                if let Some(frame_obj) = frame_result {
                    if !frame_obj.is_null() {
                        let array: jni::objects::JByteArray = frame_obj.into();
                        if let Ok(bytes) = env.convert_byte_array(&array) {
                            // Get dimensions
                            let dims_result = env
                                .call_static_method(
                                    &helper_class,
                                    "getFrameDimensions",
                                    "()[I",
                                    &[],
                                )
                                .ok()
                                .and_then(|r| r.l().ok());

                            let (w, h) = if let Some(dims_obj) = dims_result {
                                let dims_array: jni::objects::JIntArray = dims_obj.into();
                                let mut buf = [0i32; 2];
                                let _ = env.get_int_array_region(&dims_array, 0, &mut buf);
                                (buf[0] as u32, buf[1] as u32)
                            } else {
                                (width, height)
                            };

                            let raw = RawFrame {
                                data: bytes,
                                width: w,
                                height: h,
                                timestamp_ns: std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map_or(0, |d| d.as_nanos() as u64),
                            };
                            let _ = sender.try_send(raw);
                        }
                    }
                }

                std::thread::sleep(std::time::Duration::from_millis(16)); // ~60fps
            }

            // Stop capture when done
            if let Ok(helper) = get_helper_class(&mut env) {
                let _ = env.call_static_method(&helper, "stopCapture", "()V", &[]);
            }
        });

        Ok(Self {
            device,
            queue,
            width,
            height,
            running,
            frame_receiver: receiver,
        })
    }

    pub async fn next_frame(&self) -> Option<ScreenFrame> {
        let raw = self.frame_receiver.recv().await.ok()?;
        Some(self.create_frame(raw))
    }

    pub fn try_next_frame(&self) -> Option<ScreenFrame> {
        let raw = self.frame_receiver.try_recv().ok()?;
        Some(self.create_frame(raw))
    }

    fn create_frame(&self, raw: RawFrame) -> ScreenFrame {
        // Create GPU texture
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ScreenFrame"),
            size: wgpu::Extent3d {
                width: raw.width,
                height: raw.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Upload frame data to GPU
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &raw.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(raw.width * 4),
                rows_per_image: Some(raw.height),
            },
            wgpu::Extent3d {
                width: raw.width,
                height: raw.height,
                depth_or_array_layers: 1,
            },
        );

        ScreenFrame::from_texture(
            Arc::new(texture),
            raw.width,
            raw.height,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            raw.timestamp_ns,
        )
    }

    pub const fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

impl Drop for ScreenStreamInner {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
    }
}
