//! Android platform implementation using `MediaProjection` API.
//!
//! Screen capture on Android requires:
//!
//! 1. Call `init()` with the application context
//! 2. Request permission via Activity (call `ScreenHelper.getPermissionIntent()` from Kotlin)
//! 3. Pass the result to `onPermissionResult()`
//! 4. Call `startCapture()` to begin screen capture

use crate::frame::ScreenFrame;
use crate::screenshot::{ImageFormat, Screenshot};
use crate::stream::StreamConfig;
use crate::{Error, ScreenInfo};
use jni::objects::{Global, JByteArray, JClass, JIntArray, JObject, JString, JValue};
use jni::{Env, JavaVM, jni_sig, jni_str};
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use wgpu::{Device, Queue};

const NANOS_PER_SECOND: u64 = 1_000_000_000;

/// Embedded DEX bytecode containing `ScreenHelper` class.
/// Generated at build time by kotlinc + D8.
static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

/// Cached class loader for the embedded DEX.
static CLASS_LOADER: OnceLock<Global<JObject<'static>>> = OnceLock::new();

/// Cached application context.
static CONTEXT: OnceLock<Global<JObject<'static>>> = OnceLock::new();

fn get_vm() -> Result<JavaVM, Error> {
    let android_context = ndk_context::android_context();
    let raw_vm: *mut jni::sys::JavaVM = android_context.vm().cast();
    if raw_vm.is_null() {
        return Err(Error::Platform("ndk_context returned null JavaVM".into()));
    }
    Ok(unsafe { JavaVM::from_raw(raw_vm) })
}

fn with_attached_env<T>(
    operation: impl FnOnce(&mut Env<'_>) -> Result<T, Error>,
) -> Result<T, Error> {
    get_vm()?.attach_current_thread(operation)
}

fn ensure_dex_loaded() -> Result<(), Error> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }

    let android_context = ndk_context::android_context();
    let raw_context: jni::sys::jobject = android_context.context().cast();
    if raw_context.is_null() {
        return Err(Error::Platform("ndk_context returned null Context".into()));
    }
    with_attached_env(|env| {
        let context = unsafe { env.as_cast_raw::<JObject>(&raw_context)? };
        init_with_context(env, &context)
    })
}

fn init_with_context(env: &mut Env<'_>, context: &JObject) -> Result<(), Error> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }

    // Write DEX to cache directory
    let cache_dir = env
        .call_method(
            context,
            jni_str!("getCacheDir"),
            jni_sig!("()Ljava/io/File;"),
            &[],
        )
        .map_err(|e| Error::Platform(format!("getCacheDir: {e}")))?
        .l()
        .map_err(|e| Error::Platform(format!("getCacheDir result: {e}")))?;

    let cache_path = env
        .call_method(
            &cache_dir,
            jni_str!("getAbsolutePath"),
            jni_sig!("()Ljava/lang/String;"),
            &[],
        )
        .map_err(|e| Error::Platform(format!("getAbsolutePath: {e}")))?
        .l()
        .map_err(|e| Error::Platform(format!("getAbsolutePath result: {e}")))?;

    let cache_path_string = env
        .as_cast::<JString>(&cache_path)
        .and_then(|path| path.try_to_string(env))
        .map_err(|e| Error::Platform(format!("decode cache path: {e}")))?;
    let dex_path = format!("{cache_path_string}/waterkit_screen.dex");

    // Write DEX bytes to file
    std::fs::write(&dex_path, DEX_BYTES).map_err(|e| Error::Platform(format!("write DEX: {e}")))?;

    // Create DexClassLoader
    let dex_path_jstring = env
        .new_string(&dex_path)
        .map_err(|e| Error::Platform(format!("new_string: {e}")))?;

    let parent_loader = env
        .call_method(
            context,
            jni_str!("getClassLoader"),
            jni_sig!("()Ljava/lang/ClassLoader;"),
            &[],
        )
        .map_err(|e| Error::Platform(format!("getClassLoader: {e}")))?
        .l()
        .map_err(|e| Error::Platform(format!("getClassLoader result: {e}")))?;

    let dex_class_loader_class = env
        .find_class(jni_str!("dalvik/system/DexClassLoader"))
        .map_err(|e| Error::Platform(format!("find DexClassLoader: {e}")))?;

    let class_loader = env
        .new_object(
            dex_class_loader_class,
            jni_sig!(
                "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/ClassLoader;)V"
            ),
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
        jni_str!("initWithContext"),
        jni_sig!("(Landroid/content/Context;)Z"),
        &[JValue::Object(context)],
    )
    .map_err(|e| Error::Platform(format!("initWithContext: {e}")))?;

    Ok(())
}

/// Get the `ScreenHelper` class.
fn get_helper_class<'local>(env: &mut Env<'local>) -> Result<JClass<'local>, Error> {
    let loader = CLASS_LOADER
        .get()
        .ok_or_else(|| Error::Platform("Class loader not initialized".into()))?;

    let class_name = env
        .new_string("waterkit.screen.ScreenHelper")
        .map_err(|e| Error::Platform(format!("new_string: {e}")))?;

    let loaded_class = env
        .call_method(
            loader.as_obj(),
            jni_str!("loadClass"),
            jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
            &[JValue::Object(&class_name)],
        )
        .map_err(|e| Error::Platform(format!("loadClass: {e}")))?
        .l()
        .map_err(|e| Error::Platform(format!("loadClass result: {e}")))?;

    env.cast_local::<JClass>(loaded_class).map_err(Error::from)
}

/// Initialize the screen module with Android context.
pub fn init(env: &mut Env<'_>, context: &JObject) -> Result<(), Error> {
    let global = env
        .new_global_ref(context)
        .map_err(|e| Error::Platform(e.to_string()))?;
    let _ = CONTEXT.set(global);
    init_with_context(env, context)
}

/// Enumerate screens (returns single main screen with actual dimensions).
pub fn screens() -> Result<Vec<ScreenInfo>, Error> {
    ensure_dex_loaded()?;
    with_attached_env(|env| {
        let helper_class = get_helper_class(env)?;
        let dims = env
            .call_static_method(
                &helper_class,
                jni_str!("getFrameDimensions"),
                jni_sig!("()[I"),
                &[],
            )
            .map_err(|e| Error::Platform(format!("getFrameDimensions: {e}")))?
            .l()
            .map_err(|e| Error::Platform(format!("getFrameDimensions result: {e}")))?;

        let dims_array = env.cast_local::<JIntArray>(dims)?;
        let mut dims_buf = [0i32; 2];
        dims_array
            .get_region(env, 0, &mut dims_buf)
            .map_err(|e| Error::Platform(format!("read frame dimensions: {e}")))?;
        let width = u32::try_from(dims_buf[0]).map_err(|_| {
            Error::Platform(format!(
                "Android reported invalid screen width: {}",
                dims_buf[0]
            ))
        })?;
        let height = u32::try_from(dims_buf[1]).map_err(|_| {
            Error::Platform(format!(
                "Android reported invalid screen height: {}",
                dims_buf[1]
            ))
        })?;
        if width == 0 || height == 0 {
            return Err(Error::Platform(format!(
                "Android reported zero screen dimensions: {width}x{height}"
            )));
        }

        Ok(vec![ScreenInfo::new(
            0,
            "Main Screen".into(),
            width,
            height,
            1.0,
            true,
        )])
    })
}

/// Return the maximum refresh rate reported by Android display metadata.
pub fn max_refresh_rate_hz() -> Result<f32, Error> {
    ensure_dex_loaded()?;
    let refresh_hz = with_attached_env(|env| {
        let helper_class = get_helper_class(env)?;
        env.call_static_method(
            &helper_class,
            jni_str!("getRefreshRateHz"),
            jni_sig!("()F"),
            &[],
        )
        .map_err(|e| Error::Platform(format!("getRefreshRateHz: {e}")))?
        .f()
        .map_err(|e| Error::Platform(format!("getRefreshRateHz result: {e}")))
    })?;

    if refresh_hz.is_finite() && refresh_hz > 0.0 {
        Ok(refresh_hz)
    } else {
        Err(Error::Platform(format!(
            "invalid Android refresh rate value: {refresh_hz}"
        )))
    }
}

/// Capture a screenshot on Android using `MediaProjection`.
pub fn screenshot(display: &ScreenInfo, format: ImageFormat) -> Result<Screenshot, Error> {
    if !matches!(format, ImageFormat::Png) {
        return Err(Error::Unsupported);
    }

    ensure_dex_loaded()?;
    let data = with_attached_env(|env| {
        let helper_class = get_helper_class(env)?;
        let has_permission = env
            .call_static_method(
                &helper_class,
                jni_str!("hasPermission"),
                jni_sig!("()Z"),
                &[],
            )
            .map_err(|e| Error::Platform(format!("hasPermission: {e}")))?
            .z()
            .map_err(|e| Error::Platform(format!("hasPermission result: {e}")))?;

        if !has_permission {
            return Err(Error::PermissionDenied);
        }

        let screenshot_obj = env
            .call_static_method(
                &helper_class,
                jni_str!("captureScreenshotPng"),
                jni_sig!("()[B"),
                &[],
            )
            .map_err(|e| Error::Platform(format!("captureScreenshotPng: {e}")))?
            .l()
            .map_err(|e| Error::Platform(format!("captureScreenshotPng result: {e}")))?;

        if screenshot_obj.is_null() {
            return Err(Error::Platform(
                "captureScreenshotPng returned null frame data".into(),
            ));
        }

        let bytes_array = env.cast_local::<JByteArray>(screenshot_obj)?;
        let data = env
            .convert_byte_array(&bytes_array)
            .map_err(|e| Error::Platform(format!("convert_byte_array: {e}")))?;
        if data.is_empty() {
            return Err(Error::Platform(
                "captureScreenshotPng returned empty data".into(),
            ));
        }
        Ok(data)
    })?;

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
    with_attached_env(|env| {
        let helper_class = get_helper_class(env)?;
        env.call_static_method(
            &helper_class,
            jni_str!("getBrightness"),
            jni_sig!("()F"),
            &[],
        )
        .map_err(|e| Error::Platform(format!("getBrightness: {e}")))?
        .f()
        .map_err(|e| Error::Platform(format!("getBrightness result: {e}")))
    })
}

/// Set screen brightness.
#[allow(clippy::unused_async)]
pub async fn set_brightness(val: f32) -> Result<(), Error> {
    ensure_dex_loaded()?;
    with_attached_env(|env| {
        let helper_class = get_helper_class(env)?;
        let result = env
            .call_static_method(
                &helper_class,
                jni_str!("setBrightness"),
                jni_sig!("(F)Z"),
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
    })
}

/// Raw frame data from Android `MediaProjection`.
struct RawFrame {
    data: Vec<u8>,
    width: u32,
    height: u32,
    timestamp_ns: u64,
}

fn take_frame_with_context(env: &mut Env<'_>) -> Result<Option<RawFrame>, Error> {
    let helper_class = get_helper_class(env)?;
    let frame_obj = env
        .call_static_method(&helper_class, jni_str!("getFrame"), jni_sig!("()[B"), &[])
        .map_err(|error| Error::Platform(format!("getFrame: {error}")))?
        .l()
        .map_err(|error| Error::Platform(format!("getFrame result: {error}")))?;
    if frame_obj.is_null() {
        return Ok(None);
    }

    let array = env.cast_local::<JByteArray>(frame_obj)?;
    let data = env
        .convert_byte_array(&array)
        .map_err(|error| Error::Platform(format!("convert frame byte array: {error}")))?;
    let dims_obj = env
        .call_static_method(
            &helper_class,
            jni_str!("getFrameDimensions"),
            jni_sig!("()[I"),
            &[],
        )
        .map_err(|error| Error::Platform(format!("getFrameDimensions: {error}")))?
        .l()
        .map_err(|error| Error::Platform(format!("getFrameDimensions result: {error}")))?;

    if dims_obj.is_null() {
        return Err(Error::Platform("getFrameDimensions returned null".into()));
    }
    let dims_array = env.cast_local::<JIntArray>(dims_obj)?;
    let mut dimensions = [0i32; 2];
    dims_array
        .get_region(env, 0, &mut dimensions)
        .map_err(|error| Error::Platform(format!("read frame dimensions: {error}")))?;
    let width = u32::try_from(dimensions[0]).map_err(|_| {
        Error::Platform(format!(
            "Android reported invalid frame width: {}",
            dimensions[0]
        ))
    })?;
    let height = u32::try_from(dimensions[1]).map_err(|_| {
        Error::Platform(format!(
            "Android reported invalid frame height: {}",
            dimensions[1]
        ))
    })?;
    if width == 0 || height == 0 {
        return Err(Error::Platform(format!(
            "Android reported zero frame dimensions: {width}x{height}"
        )));
    }
    let timestamp_ns = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| Error::Platform(format!("system clock before Unix epoch: {error}")))?
            .as_nanos(),
    )
    .map_err(|_| Error::Platform("frame timestamp exceeds u64 nanoseconds".into()))?;

    Ok(Some(RawFrame {
        data,
        width,
        height,
        timestamp_ns,
    }))
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
        config: &StreamConfig,
    ) -> Result<Self, Error> {
        ensure_dex_loaded()?;
        let frame_interval = frame_interval(config.target_fps)?;

        with_attached_env(|env| {
            let helper_class = get_helper_class(env)?;
            let has_permission = env
                .call_static_method(
                    &helper_class,
                    jni_str!("hasPermission"),
                    jni_sig!("()Z"),
                    &[],
                )
                .map_err(|e| Error::Platform(format!("hasPermission: {e}")))?
                .z()
                .map_err(|e| Error::Platform(format!("hasPermission result: {e}")))?;

            if !has_permission {
                return Err(Error::PermissionDenied);
            }

            let started = env
                .call_static_method(
                    &helper_class,
                    jni_str!("startCapture"),
                    jni_sig!("()Z"),
                    &[],
                )
                .map_err(|e| Error::Platform(format!("startCapture: {e}")))?
                .z()
                .map_err(|e| Error::Platform(format!("startCapture result: {e}")))?;

            if started {
                Ok(())
            } else {
                Err(Error::Platform("Failed to start screen capture".into()))
            }
        })?;

        let (sender, receiver) = async_channel::bounded(2);
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();
        let width = display.width();
        let height = display.height();
        let capture_vm = get_vm()?;

        // Spawn frame capture thread
        std::thread::spawn(move || {
            let result = capture_vm.attach_current_thread(|env| -> Result<(), Error> {
                while running_clone.load(Ordering::SeqCst) {
                    if let Some(frame) = take_frame_with_context(env)? {
                        match sender.try_send(frame) {
                            Ok(()) | Err(async_channel::TrySendError::Full(_)) => {}
                            Err(async_channel::TrySendError::Closed(_)) => break,
                        }
                    }
                    std::thread::sleep(frame_interval);
                }

                let helper = get_helper_class(env)?;
                env.call_static_method(&helper, jni_str!("stopCapture"), jni_sig!("()V"), &[])
                    .map_err(|error| Error::Platform(format!("stopCapture: {error}")))?;
                Ok(())
            });
            if let Err(error) = result {
                tracing::error!(%error, "Android screen capture thread failed");
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
        Some(self.create_frame(&raw))
    }

    pub fn try_next_frame(&self) -> Option<ScreenFrame> {
        let raw = self.frame_receiver.try_recv().ok()?;
        Some(self.create_frame(&raw))
    }

    fn create_frame(&self, raw: &RawFrame) -> ScreenFrame {
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

fn frame_interval(target_fps: u32) -> Result<Duration, Error> {
    if target_fps == 0 {
        return Err(Error::Platform(
            "target_fps must be greater than zero".into(),
        ));
    }

    NonZeroU64::new(NANOS_PER_SECOND / u64::from(target_fps))
        .map(|nanos| Duration::from_nanos(nanos.get()))
        .ok_or_else(|| Error::Platform(format!("target_fps is too high: {target_fps}")))
}

impl Drop for ScreenStreamInner {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
    }
}
