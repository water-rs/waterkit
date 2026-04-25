use crate::VideoError;
#[cfg(any(target_os = "ios", target_os = "macos"))]
use core::ffi::c_void;
use core::num::NonZeroU64;
use std::time::Duration;

/// Stable identifier for one picture-in-picture host registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PictureInPictureHostId(NonZeroU64);

impl PictureInPictureHostId {
    /// Create a new host id.
    #[must_use]
    pub const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    /// Returns the raw non-zero host id.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// Platform picture-in-picture controller state for the current player instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PictureInPictureControllerState {
    /// Stable host id for the current render source.
    pub host_id: PictureInPictureHostId,
    /// Whether the player is currently able to participate in picture in picture.
    pub active: bool,
    /// Whether playback is currently active.
    pub playing: bool,
    /// Display aspect ratio for the active video when known.
    pub aspect_ratio: Option<(u32, u32)>,
}

impl PictureInPictureControllerState {
    /// Create a new controller state.
    #[must_use]
    pub const fn new(
        host_id: PictureInPictureHostId,
        active: bool,
        playing: bool,
        aspect_ratio: Option<(u32, u32)>,
    ) -> Self {
        Self {
            host_id,
            active,
            playing,
            aspect_ratio,
        }
    }
}

/// Commands emitted by picture-in-picture playback controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PictureInPictureCommand {
    /// Request playback to start or resume.
    Play,
    /// Request playback to pause.
    Pause,
    /// Request seeking forward by the specified amount.
    SeekForward(Duration),
    /// Request seeking backward by the specified amount.
    SeekBackward(Duration),
}

/// Callback that renders the current frame into an external Metal texture.
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub type ApplePictureInPictureRenderFrame =
    unsafe extern "C" fn(*mut c_void, *mut c_void, u32, u32) -> bool;

/// Callback that toggles long-lived external rendering for the host surface.
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub type ApplePictureInPictureSetExternalRendering = unsafe extern "C" fn(*mut c_void, bool);

/// Request picture in picture for the specified host.
///
/// `aspect_ratio` uses the displayed video aspect ratio when known.
///
/// # Errors
///
/// Returns an error when the current platform does not support picture in picture
/// or the host is not registered/configured for it.
pub fn enter_picture_in_picture(
    host_id: PictureInPictureHostId,
    aspect_ratio: Option<(u32, u32)>,
) -> Result<(), VideoError> {
    #[cfg(target_os = "android")]
    {
        android::enter_picture_in_picture(host_id, aspect_ratio)
    }

    #[cfg(any(target_os = "ios", target_os = "macos"))]
    {
        apple::enter_picture_in_picture(host_id, aspect_ratio)
    }

    #[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
    {
        let _ = host_id;
        let _ = aspect_ratio;
        Err(VideoError::Unsupported(
            "picture in picture is unavailable on this platform".into(),
        ))
    }
}

/// Synchronize the picture-in-picture controller with current playback state.
///
/// # Errors
///
/// Returns an error when the current platform helper cannot be reached.
pub fn sync_picture_in_picture_controller(
    state: PictureInPictureControllerState,
) -> Result<(), VideoError> {
    #[cfg(target_os = "android")]
    {
        android::sync_picture_in_picture_controller(state)
    }

    #[cfg(any(target_os = "ios", target_os = "macos"))]
    {
        apple::sync_picture_in_picture_controller(state);
        Ok(())
    }

    #[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
    {
        let _ = state;
        Err(VideoError::Unsupported(
            "picture in picture controller sync is unavailable on this platform".into(),
        ))
    }
}

/// Returns whether the specified host is currently in picture in picture.
///
/// # Errors
///
/// Returns an error when the current platform helper cannot be reached.
pub fn is_picture_in_picture_active(host_id: PictureInPictureHostId) -> Result<bool, VideoError> {
    #[cfg(target_os = "android")]
    {
        let _ = host_id;
        android::is_picture_in_picture_active()
    }

    #[cfg(any(target_os = "ios", target_os = "macos"))]
    {
        Ok(apple::is_picture_in_picture_active(host_id))
    }

    #[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
    {
        let _ = host_id;
        Err(VideoError::Unsupported(
            "picture in picture state query is unavailable on this platform".into(),
        ))
    }
}

/// Poll one pending picture-in-picture playback command for the specified host.
///
/// Returns `Ok(None)` when the platform exposes no extra picture-in-picture playback commands.
///
/// # Errors
///
/// Returns an error when the current platform helper cannot be reached.
pub fn poll_picture_in_picture_command(
    host_id: PictureInPictureHostId,
) -> Result<Option<PictureInPictureCommand>, VideoError> {
    #[cfg(target_os = "android")]
    {
        let _ = host_id;
        Ok(None)
    }

    #[cfg(any(target_os = "ios", target_os = "macos"))]
    {
        apple::poll_picture_in_picture_command(host_id)
    }

    #[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
    {
        let _ = host_id;
        Ok(None)
    }
}

/// Register an Apple `GpuSurface` host that can render frames for picture in picture.
///
/// This is called by the Apple backend and should not be used directly from app code.
#[cfg(any(target_os = "ios", target_os = "macos"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn waterkit_video_apple_register_gpu_surface_host(
    host_id: u64,
    user_data: *mut c_void,
    render_frame: ApplePictureInPictureRenderFrame,
    set_external_rendering: ApplePictureInPictureSetExternalRendering,
) {
    let host_id = PictureInPictureHostId::new(
        NonZeroU64::new(host_id).expect("waterkit-video apple host id must be non-zero"),
    );
    apple::register_gpu_surface_host(host_id, user_data, render_frame, set_external_rendering);
}

/// Unregister an Apple `GpuSurface` host previously registered for picture in picture.
///
/// This is called by the Apple backend and should not be used directly from app code.
#[cfg(any(target_os = "ios", target_os = "macos"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn waterkit_video_apple_unregister_gpu_surface_host(host_id: u64) {
    let host_id = PictureInPictureHostId::new(
        NonZeroU64::new(host_id).expect("waterkit-video apple host id must be non-zero"),
    );
    apple::unregister_gpu_surface_host(host_id);
}

#[cfg(target_os = "android")]
mod android {
    use super::{PictureInPictureControllerState, PictureInPictureHostId};
    use crate::VideoError;
    use jni::{
        JNIEnv, JavaVM,
        objects::{GlobalRef, JClass, JObject, JValue},
    };
    use std::{convert::TryFrom, mem::ManuallyDrop, sync::OnceLock};

    static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));
    static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();

    const PICTURE_IN_PICTURE_HELPER_CLASS: &str = "waterkit.video.PictureInPictureHelper";
    const RESULT_ENTERED: i32 = 0;
    const RESULT_PLATFORM_UNSUPPORTED: i32 = 1;
    const RESULT_DEVICE_UNSUPPORTED: i32 = 2;
    const RESULT_ACTIVITY_UNAVAILABLE: i32 = 3;
    const RESULT_ACTIVITY_NOT_DECLARED: i32 = 4;
    const RESULT_ENTER_FAILED: i32 = 5;

    pub(super) fn sync_picture_in_picture_controller(
        state: PictureInPictureControllerState,
    ) -> Result<(), VideoError> {
        let _ = state.host_id;
        with_android_context(|env, context| {
            let helper_class = helper_class(env, context)?;
            let (aspect_width, aspect_height) = aspect_ratio_components(state.aspect_ratio)?;

            env.call_static_method(
                helper_class,
                "updateControllerState",
                "(Landroid/content/Context;ZZII)V",
                &[
                    JValue::Object(context),
                    JValue::Bool(u8::from(state.active)),
                    JValue::Bool(u8::from(state.playing)),
                    JValue::Int(aspect_width),
                    JValue::Int(aspect_height),
                ],
            )
            .map_err(|error| {
                VideoError::Unsupported(format!(
                    "Android picture in picture controller sync failed: {error}"
                ))
            })?;

            Ok(())
        })
    }

    pub(super) fn enter_picture_in_picture(
        _host_id: PictureInPictureHostId,
        aspect_ratio: Option<(u32, u32)>,
    ) -> Result<(), VideoError> {
        with_android_context(|env, context| {
            let helper_class = helper_class(env, context)?;
            let (aspect_width, aspect_height) = aspect_ratio_components(aspect_ratio)?;

            let result = env
                .call_static_method(
                    helper_class,
                    "enterPictureInPicture",
                    "(Landroid/content/Context;II)I",
                    &[
                        JValue::Object(context),
                        JValue::Int(aspect_width),
                        JValue::Int(aspect_height),
                    ],
                )
                .map_err(|error| {
                    VideoError::Unsupported(format!(
                        "Android picture in picture helper call failed: {error}"
                    ))
                })?
                .i()
                .map_err(|error| {
                    VideoError::Unsupported(format!(
                        "Android picture in picture helper returned invalid result: {error}"
                    ))
                })?;

            match result {
                RESULT_ENTERED => Ok(()),
                RESULT_PLATFORM_UNSUPPORTED => Err(VideoError::Unsupported(
                    "picture in picture requires Android 8.0 or newer".into(),
                )),
                RESULT_DEVICE_UNSUPPORTED => Err(VideoError::Unsupported(
                    "device does not support picture in picture".into(),
                )),
                RESULT_ACTIVITY_UNAVAILABLE => Err(VideoError::Unsupported(
                    "no active Android activity is available for picture in picture".into(),
                )),
                RESULT_ACTIVITY_NOT_DECLARED => Err(VideoError::Unsupported(
                    "host Android activity must declare supportsPictureInPicture=true".into(),
                )),
                RESULT_ENTER_FAILED => Err(VideoError::Unsupported(
                    "Android activity rejected the picture in picture request".into(),
                )),
                _ => Err(VideoError::Unsupported(format!(
                    "Android picture in picture helper returned unknown result code {result}"
                ))),
            }
        })
    }

    pub(super) fn is_picture_in_picture_active() -> Result<bool, VideoError> {
        with_android_context(|env, context| {
            let helper_class = helper_class(env, context)?;
            env.call_static_method(
                helper_class,
                "isPictureInPictureActive",
                "(Landroid/content/Context;)Z",
                &[JValue::Object(context)],
            )
            .map_err(|error| {
                VideoError::Unsupported(format!(
                    "Android picture in picture state query failed: {error}"
                ))
            })?
            .z()
            .map_err(|error| {
                VideoError::Unsupported(format!(
                    "Android picture in picture state query returned invalid result: {error}"
                ))
            })
        })
    }

    pub(super) fn notify_user_leave_hint_with_context(
        env: &mut JNIEnv<'_>,
        context: &JObject<'_>,
    ) -> Result<(), VideoError> {
        let helper_class = helper_class(env, context)?;
        env.call_static_method(
            helper_class,
            "onUserLeaveHint",
            "(Landroid/content/Context;)V",
            &[JValue::Object(context)],
        )
        .map_err(|error| {
            VideoError::Unsupported(format!(
                "Android picture in picture onUserLeaveHint failed: {error}"
            ))
        })?;
        Ok(())
    }

    fn init_with_context(env: &mut JNIEnv<'_>, context: &JObject<'_>) -> Result<(), VideoError> {
        if CLASS_LOADER.get().is_some() {
            return Ok(());
        }

        let cache_dir = env
            .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])
            .map_err(|error| {
                VideoError::Unsupported(format!(
                    "Android picture in picture getCacheDir failed: {error}"
                ))
            })?
            .l()
            .map_err(|error| {
                VideoError::Unsupported(format!(
                    "Android picture in picture getCacheDir result failed: {error}"
                ))
            })?;

        let cache_path = env
            .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
            .map_err(|error| {
                VideoError::Unsupported(format!(
                    "Android picture in picture cache path lookup failed: {error}"
                ))
            })?
            .l()
            .map_err(|error| {
                VideoError::Unsupported(format!(
                    "Android picture in picture cache path result failed: {error}"
                ))
            })?;

        let dex_path = format!(
            "{}/waterkit_video_picture_in_picture.dex",
            env.get_string((&cache_path).into())
                .map_err(|error| {
                    VideoError::Unsupported(format!(
                        "Android picture in picture cache path string failed: {error}"
                    ))
                })?
                .to_str()
                .map_err(|error| {
                    VideoError::Unsupported(format!(
                        "Android picture in picture cache path UTF-8 failed: {error}"
                    ))
                })?
        );
        std::fs::write(&dex_path, DEX_BYTES).map_err(|error| {
            VideoError::Unsupported(format!(
                "Android picture in picture DEX write failed: {error}"
            ))
        })?;

        let dex_path_jstring = env.new_string(&dex_path).map_err(|error| {
            VideoError::Unsupported(format!(
                "Android picture in picture dex path string failed: {error}"
            ))
        })?;

        let parent_loader = env
            .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
            .map_err(|error| {
                VideoError::Unsupported(format!(
                    "Android picture in picture parent class loader failed: {error}"
                ))
            })?
            .l()
            .map_err(|error| {
                VideoError::Unsupported(format!(
                    "Android picture in picture parent class loader result failed: {error}"
                ))
            })?;

        let dex_class_loader_class =
            env.find_class("dalvik/system/DexClassLoader")
                .map_err(|error| {
                    VideoError::Unsupported(format!(
                        "Android picture in picture DexClassLoader lookup failed: {error}"
                    ))
                })?;

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
            .map_err(|error| {
                VideoError::Unsupported(format!(
                    "Android picture in picture DexClassLoader construction failed: {error}"
                ))
            })?;

        let global_ref = env.new_global_ref(class_loader).map_err(|error| {
            VideoError::Unsupported(format!(
                "Android picture in picture class loader global ref failed: {error}"
            ))
        })?;
        let _ = CLASS_LOADER.set(global_ref);
        Ok(())
    }

    fn helper_class<'env>(
        env: &mut JNIEnv<'env>,
        context: &JObject<'_>,
    ) -> Result<JClass<'env>, VideoError> {
        init_with_context(env, context)?;

        let class_loader = CLASS_LOADER.get().ok_or_else(|| {
            VideoError::Unsupported(
                "Android picture in picture class loader was not initialized".into(),
            )
        })?;
        let class_name = env
            .new_string(PICTURE_IN_PICTURE_HELPER_CLASS)
            .map_err(|error| {
                VideoError::Unsupported(format!(
                    "Android picture in picture helper class name failed: {error}"
                ))
            })?;
        let helper_class = env
            .call_method(
                class_loader.as_obj(),
                "loadClass",
                "(Ljava/lang/String;)Ljava/lang/Class;",
                &[JValue::Object(&class_name)],
            )
            .map_err(|error| {
                VideoError::Unsupported(format!(
                    "Android picture in picture helper loadClass failed: {error}"
                ))
            })?
            .l()
            .map_err(|error| {
                VideoError::Unsupported(format!(
                    "Android picture in picture helper loadClass result failed: {error}"
                ))
            })?;
        Ok(helper_class.into())
    }

    fn aspect_ratio_components(aspect_ratio: Option<(u32, u32)>) -> Result<(i32, i32), VideoError> {
        let (aspect_width, aspect_height) = aspect_ratio.unwrap_or((0, 0));
        let aspect_width = i32::try_from(aspect_width).map_err(|_| {
            VideoError::Unsupported("picture in picture width exceeds Android jint range".into())
        })?;
        let aspect_height = i32::try_from(aspect_height).map_err(|_| {
            VideoError::Unsupported("picture in picture height exceeds Android jint range".into())
        })?;
        Ok((aspect_width, aspect_height))
    }

    fn with_android_context<T>(
        f: impl FnOnce(&mut JNIEnv<'_>, &JObject<'_>) -> Result<T, VideoError>,
    ) -> Result<T, VideoError> {
        let android_context = ndk_context::android_context();
        let vm = unsafe { JavaVM::from_raw(android_context.vm().cast()) }.map_err(|error| {
            VideoError::Unsupported(format!("JavaVM::from_raw failed: {error}"))
        })?;

        let mut env = vm.attach_current_thread().map_err(|error| {
            VideoError::Unsupported(format!("attach_current_thread failed: {error}"))
        })?;

        let context =
            ManuallyDrop::new(unsafe { JObject::from_raw(android_context.context().cast()) });
        assert!(
            !context.is_null(),
            "waterkit-video: ndk_context returned null Android Context"
        );

        init_with_context(&mut env, &context)?;
        f(&mut env, &context)
    }
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_waterui_android_ffi_WatcherJni_notifyVideoPictureInPictureUserLeaveHint(
    mut env: jni::JNIEnv<'_>,
    _class: jni::objects::JClass<'_>,
    context: jni::objects::JObject<'_>,
) {
    android::notify_user_leave_hint_with_context(&mut env, &context)
        .unwrap_or_else(|error| panic!("waterkit-video Android onUserLeaveHint failed: {error}"));
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple {
    use super::{
        ApplePictureInPictureRenderFrame, ApplePictureInPictureSetExternalRendering,
        PictureInPictureCommand, PictureInPictureControllerState, PictureInPictureHostId,
    };
    use crate::VideoError;
    use core::ffi::c_void;
    use std::time::Duration;

    const RESULT_SUCCESS: i32 = 0;
    const RESULT_UNSUPPORTED: i32 = 1;
    const RESULT_HOST_NOT_REGISTERED: i32 = 2;
    const RESULT_NOT_POSSIBLE: i32 = 3;
    const RESULT_START_FAILED: i32 = 4;

    const COMMAND_NONE: i32 = 0;
    const COMMAND_PLAY: i32 = 1;
    const COMMAND_PAUSE: i32 = 2;
    const COMMAND_SEEK_FORWARD: i32 = 3;
    const COMMAND_SEEK_BACKWARD: i32 = 4;

    unsafe extern "C" {
        fn waterkit_video_apple_pip_bridge_register_host(
            host_id: u64,
            user_data: *mut c_void,
            render_frame: ApplePictureInPictureRenderFrame,
            set_external_rendering: ApplePictureInPictureSetExternalRendering,
        );
        fn waterkit_video_apple_pip_bridge_unregister_host(host_id: u64);
        fn waterkit_video_apple_pip_bridge_sync_host_state(
            host_id: u64,
            active: bool,
            playing: bool,
            aspect_width: u32,
            aspect_height: u32,
        );
        fn waterkit_video_apple_pip_bridge_enter(host_id: u64) -> i32;
        fn waterkit_video_apple_pip_bridge_is_active(host_id: u64) -> bool;
        fn waterkit_video_apple_pip_bridge_poll_command_kind(
            host_id: u64,
            kind_out: *mut i32,
            value_secs_out: *mut f64,
        );
    }

    pub(super) fn register_gpu_surface_host(
        host_id: PictureInPictureHostId,
        user_data: *mut c_void,
        render_frame: ApplePictureInPictureRenderFrame,
        set_external_rendering: ApplePictureInPictureSetExternalRendering,
    ) {
        unsafe {
            waterkit_video_apple_pip_bridge_register_host(
                host_id.get(),
                user_data,
                render_frame,
                set_external_rendering,
            );
        }
    }

    pub(super) fn unregister_gpu_surface_host(host_id: PictureInPictureHostId) {
        unsafe {
            waterkit_video_apple_pip_bridge_unregister_host(host_id.get());
        }
    }

    pub(super) fn sync_picture_in_picture_controller(state: PictureInPictureControllerState) {
        let (aspect_width, aspect_height) = state.aspect_ratio.unwrap_or((0, 0));
        unsafe {
            waterkit_video_apple_pip_bridge_sync_host_state(
                state.host_id.get(),
                state.active,
                state.playing,
                aspect_width,
                aspect_height,
            );
        }
    }

    pub(super) fn enter_picture_in_picture(
        host_id: PictureInPictureHostId,
        _aspect_ratio: Option<(u32, u32)>,
    ) -> Result<(), VideoError> {
        let result = unsafe { waterkit_video_apple_pip_bridge_enter(host_id.get()) };
        match result {
            RESULT_SUCCESS => Ok(()),
            RESULT_UNSUPPORTED => Err(VideoError::Unsupported(
                "Apple picture in picture is unavailable on this device".into(),
            )),
            RESULT_HOST_NOT_REGISTERED => Err(VideoError::Unsupported(
                "Apple picture in picture host has not been registered".into(),
            )),
            RESULT_NOT_POSSIBLE => Err(VideoError::Unsupported(
                "Apple picture in picture is currently not possible for this host".into(),
            )),
            RESULT_START_FAILED => Err(VideoError::Unsupported(
                "Apple picture in picture controller rejected the start request".into(),
            )),
            _ => Err(VideoError::Unsupported(format!(
                "Apple picture in picture helper returned unknown result code {result}"
            ))),
        }
    }

    pub(super) fn is_picture_in_picture_active(host_id: PictureInPictureHostId) -> bool {
        unsafe { waterkit_video_apple_pip_bridge_is_active(host_id.get()) }
    }

    pub(super) fn poll_picture_in_picture_command(
        host_id: PictureInPictureHostId,
    ) -> Result<Option<PictureInPictureCommand>, VideoError> {
        let mut kind = COMMAND_NONE;
        let mut value_secs = 0.0;
        unsafe {
            waterkit_video_apple_pip_bridge_poll_command_kind(
                host_id.get(),
                &raw mut kind,
                &raw mut value_secs,
            );
        }
        let command = match kind {
            COMMAND_NONE => None,
            COMMAND_PLAY => Some(PictureInPictureCommand::Play),
            COMMAND_PAUSE => Some(PictureInPictureCommand::Pause),
            COMMAND_SEEK_FORWARD => Some(PictureInPictureCommand::SeekForward(
                Duration::from_secs_f64(value_secs),
            )),
            COMMAND_SEEK_BACKWARD => Some(PictureInPictureCommand::SeekBackward(
                Duration::from_secs_f64(value_secs),
            )),
            _ => {
                return Err(VideoError::Unsupported(format!(
                    "Apple picture in picture helper returned unknown command kind {kind}"
                )));
            }
        };
        Ok(command)
    }
}
