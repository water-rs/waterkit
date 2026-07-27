#[cfg(any(target_os = "ios", target_os = "macos"))]
use core::ffi::c_void;
use core::num::NonZeroU64;
use std::thread::JoinHandle;
use std::time::Duration;
use waterkit_video_core::Error as VideoError;

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
    /// Report a platform picture-in-picture lifecycle transition.
    ActiveChanged(bool),
}

/// Event-driven picture-in-picture command source for one registered host.
///
/// Apple platform callbacks are bridged through one blocking worker; rendering
/// code receives commands from the async channel without polling Swift on each
/// frame. Other platforms expose `PiP` transport through [`waterkit_audio::MediaSession`],
/// so this stream closes immediately there.
pub struct PictureInPictureCommandStream {
    host_id: PictureInPictureHostId,
    receiver: async_channel::Receiver<PictureInPictureCommand>,
    worker: Option<JoinHandle<()>>,
}

impl PictureInPictureCommandStream {
    /// Opens the command stream for a `PiP` host.
    #[must_use]
    pub fn new(host_id: PictureInPictureHostId) -> Self {
        let (sender, receiver) = async_channel::unbounded();

        #[cfg(any(target_os = "ios", target_os = "macos"))]
        let worker = {
            apple::open_picture_in_picture_command_channel(host_id);
            Some(std::thread::spawn(move || {
                loop {
                    match apple::wait_picture_in_picture_command(host_id) {
                        Ok(Some(command)) => {
                            if sender.send_blocking(command).is_err() {
                                break;
                            }
                        }
                        Ok(None) => break,
                        Err(error) => {
                            tracing::error!(%error, host_id = host_id.get(), "invalid Apple picture-in-picture command");
                            break;
                        }
                    }
                }
            }))
        };

        #[cfg(not(any(target_os = "ios", target_os = "macos")))]
        let worker = {
            drop(sender);
            None
        };

        Self {
            host_id,
            receiver,
            worker,
        }
    }

    /// Returns the event-driven command receiver.
    ///
    /// Cloned receivers distribute commands among consumers, so one player
    /// should designate exactly one command handler.
    #[must_use]
    pub fn receiver(&self) -> async_channel::Receiver<PictureInPictureCommand> {
        self.receiver.clone()
    }
}

impl std::fmt::Debug for PictureInPictureCommandStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PictureInPictureCommandStream")
            .field("host_id", &self.host_id)
            .finish_non_exhaustive()
    }
}

impl Drop for PictureInPictureCommandStream {
    fn drop(&mut self) {
        #[cfg(any(target_os = "ios", target_os = "macos"))]
        apple::close_picture_in_picture_command_channel(self.host_id);
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .expect("picture-in-picture command worker must not panic during shutdown");
        }
    }
}

/// Callback that renders the current frame into an external Metal texture.
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub type ApplePictureInPictureRenderFrame =
    unsafe extern "C" fn(*mut c_void, *mut c_void, u32, u32) -> bool;

/// Callback that toggles long-lived external rendering for the host surface.
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub type ApplePictureInPictureSetExternalRendering = unsafe extern "C" fn(*mut c_void, bool);

/// Instance-scoped platform picture-in-picture controller.
///
/// Keeping platform loader state on the player prevents hidden process-global
/// state and makes multiple player lifetimes independent.
pub struct PictureInPictureController {
    host_id: PictureInPictureHostId,
    #[cfg(target_os = "android")]
    platform: android::Controller,
}

impl PictureInPictureController {
    /// Creates a controller for one stable render host.
    #[must_use]
    pub const fn new(host_id: PictureInPictureHostId) -> Self {
        Self {
            host_id,
            #[cfg(target_os = "android")]
            platform: android::Controller::new(),
        }
    }

    /// Requests picture in picture using the displayed video aspect ratio.
    ///
    /// # Errors
    ///
    /// Returns an error when the platform does not support picture in picture
    /// or the host is not registered/configured for it.
    pub fn enter(&mut self, aspect_ratio: Option<(u32, u32)>) -> Result<(), VideoError> {
        #[cfg(target_os = "android")]
        {
            self.platform.enter(aspect_ratio)
        }

        #[cfg(any(target_os = "ios", target_os = "macos"))]
        {
            apple::enter_picture_in_picture(self.host_id, aspect_ratio)
        }

        #[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
        {
            let _ = aspect_ratio;
            Err(VideoError::Unsupported(
                "picture in picture is unavailable on this platform".into(),
            ))
        }
    }

    /// Synchronizes platform controls with current playback state.
    ///
    /// # Errors
    ///
    /// Returns an error when the platform helper cannot be reached.
    ///
    /// # Panics
    ///
    /// Panics when `state` belongs to a different picture-in-picture host.
    pub fn sync(&mut self, state: PictureInPictureControllerState) -> Result<(), VideoError> {
        assert_eq!(
            state.host_id, self.host_id,
            "picture-in-picture state must target its owning controller"
        );

        #[cfg(target_os = "android")]
        {
            self.platform.sync(state)
        }

        #[cfg(any(target_os = "ios", target_os = "macos"))]
        {
            apple::sync_picture_in_picture_controller(state);
            Ok(())
        }

        #[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
        {
            Err(VideoError::Unsupported(
                "picture in picture controller sync is unavailable on this platform".into(),
            ))
        }
    }

    /// Returns whether this host is currently in picture in picture.
    ///
    /// Android exposes this query for low-frequency activity lifecycle
    /// reconciliation. Apple delivers exact lifecycle transitions through
    /// [`PictureInPictureCommandStream`] so rendering never synchronously hops
    /// to the main thread.
    ///
    /// # Errors
    ///
    /// Returns an error when the platform helper cannot be reached or when the
    /// platform provides lifecycle events instead of a synchronous query.
    pub fn is_active(&mut self) -> Result<bool, VideoError> {
        #[cfg(target_os = "android")]
        {
            self.platform.is_active()
        }

        #[cfg(any(target_os = "ios", target_os = "macos"))]
        {
            Err(VideoError::Unsupported(
                "Apple picture in picture state is event-driven".into(),
            ))
        }

        #[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
        {
            Err(VideoError::Unsupported(
                "picture in picture state query is unavailable on this platform".into(),
            ))
        }
    }
}

impl std::fmt::Debug for PictureInPictureController {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PictureInPictureController")
            .field("host_id", &self.host_id)
            .finish_non_exhaustive()
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
    use super::PictureInPictureControllerState;
    use crate::android_surface::with_attached_env;
    use jni::{
        Env, JavaVM, jni_sig, jni_str,
        objects::{Global, JClass, JObject, JValue},
    };
    use std::convert::TryFrom;
    use waterkit_video_core::Error as VideoError;

    type GlobalObjectRef = Global<JObject<'static>>;

    const DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

    const PICTURE_IN_PICTURE_HELPER_CLASS: &str = "waterkit.video.PictureInPictureHelper";
    const RESULT_ENTERED: i32 = 0;
    const RESULT_PLATFORM_UNSUPPORTED: i32 = 1;
    const RESULT_DEVICE_UNSUPPORTED: i32 = 2;
    const RESULT_ACTIVITY_UNAVAILABLE: i32 = 3;
    const RESULT_ACTIVITY_NOT_DECLARED: i32 = 4;
    const RESULT_ENTER_FAILED: i32 = 5;

    pub(super) struct Controller {
        helper_class: Option<GlobalObjectRef>,
    }

    impl Controller {
        pub(super) const fn new() -> Self {
            Self { helper_class: None }
        }

        pub(super) fn sync(
            &mut self,
            state: PictureInPictureControllerState,
        ) -> Result<(), VideoError> {
            with_android_context(|env, context| {
                let helper_class = self.helper_class(env, context)?;
                let (aspect_width, aspect_height) = aspect_ratio_components(state.aspect_ratio)?;

                env.call_static_method(
                    helper_class,
                    jni_str!("updateControllerState"),
                    jni_sig!("(Landroid/content/Context;ZZII)V"),
                    &[
                        JValue::Object(context),
                        JValue::Bool(state.active),
                        JValue::Bool(state.playing),
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

        pub(super) fn enter(&mut self, aspect_ratio: Option<(u32, u32)>) -> Result<(), VideoError> {
            with_android_context(|env, context| {
                let helper_class = self.helper_class(env, context)?;
                let (aspect_width, aspect_height) = aspect_ratio_components(aspect_ratio)?;

                let result = env
                    .call_static_method(
                        helper_class,
                        jni_str!("enterPictureInPicture"),
                        jni_sig!("(Landroid/content/Context;II)I"),
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

        pub(super) fn is_active(&mut self) -> Result<bool, VideoError> {
            with_android_context(|env, context| {
                let helper_class = self.helper_class(env, context)?;
                env.call_static_method(
                    helper_class,
                    jni_str!("isPictureInPictureActive"),
                    jni_sig!("(Landroid/content/Context;)Z"),
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

        fn helper_class<'env>(
            &mut self,
            env: &mut Env<'env>,
            context: &JObject<'_>,
        ) -> Result<JClass<'env>, VideoError> {
            if self.helper_class.is_none() {
                self.helper_class = Some(load_helper_class(env, context)?);
            }
            let helper_class = self
                .helper_class
                .as_ref()
                .expect("initialized Android picture-in-picture class must exist");
            let local_class = env.new_local_ref(helper_class.as_obj()).map_err(|error| {
                VideoError::Unsupported(format!(
                    "Android picture in picture helper class local ref failed: {error}"
                ))
            })?;
            env.cast_local::<JClass>(local_class).map_err(|error| {
                VideoError::Unsupported(format!(
                    "Android picture in picture helper class cast failed: {error}"
                ))
            })
        }
    }

    fn load_helper_class(
        env: &mut Env<'_>,
        context: &JObject<'_>,
    ) -> Result<GlobalObjectRef, VideoError> {
        let parent_loader = env
            .call_method(
                context,
                jni_str!("getClassLoader"),
                jni_sig!("()Ljava/lang/ClassLoader;"),
                &[],
            )
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

        let dex_bytes = env.byte_array_from_slice(DEX_BYTES).map_err(|error| {
            VideoError::Unsupported(format!(
                "Android picture in picture DEX byte array failed: {error}"
            ))
        })?;
        let byte_buffer_class =
            env.find_class(jni_str!("java/nio/ByteBuffer"))
                .map_err(|error| {
                    VideoError::Unsupported(format!(
                        "Android picture in picture ByteBuffer lookup failed: {error}"
                    ))
                })?;
        let dex_buffer = env
            .call_static_method(
                byte_buffer_class,
                jni_str!("wrap"),
                jni_sig!("([B)Ljava/nio/ByteBuffer;"),
                &[JValue::Object(&dex_bytes)],
            )
            .map_err(|error| {
                VideoError::Unsupported(format!(
                    "Android picture in picture DEX ByteBuffer construction failed: {error}"
                ))
            })?
            .l()
            .map_err(|error| {
                VideoError::Unsupported(format!(
                    "Android picture in picture DEX ByteBuffer result failed: {error}"
                ))
            })?;

        let dex_class_loader_class = env
            .find_class(jni_str!("dalvik/system/InMemoryDexClassLoader"))
            .map_err(|error| {
                VideoError::Unsupported(format!(
                    "picture in picture requires Android 8.0 or newer: {error}"
                ))
            })?;

        let class_loader = env
            .new_object(
                dex_class_loader_class,
                jni_sig!("(Ljava/nio/ByteBuffer;Ljava/lang/ClassLoader;)V"),
                &[JValue::Object(&dex_buffer), JValue::Object(&parent_loader)],
            )
            .map_err(|error| {
                VideoError::Unsupported(format!(
                    "Android picture in picture in-memory class loader construction failed: {error}"
                ))
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
                &class_loader,
                jni_str!("loadClass"),
                jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
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
        env.new_global_ref(helper_class).map_err(|error| {
            VideoError::Unsupported(format!(
                "Android picture in picture helper class global ref failed: {error}"
            ))
        })
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
        f: impl FnOnce(&mut Env<'_>, &JObject<'_>) -> Result<T, VideoError>,
    ) -> Result<T, VideoError> {
        let android_context = ndk_context::android_context();
        let raw_context: jni::sys::jobject = android_context.context().cast();
        assert!(
            !raw_context.is_null(),
            "waterkit-video: ndk_context returned null Android Context"
        );
        let vm = unsafe { JavaVM::from_raw(android_context.vm().cast()) };

        with_attached_env(&vm, |env| {
            let context = unsafe { env.as_cast_raw::<JObject>(&raw_context) }.map_err(|error| {
                VideoError::Unsupported(format!(
                    "Android picture in picture context cast failed: {error}"
                ))
            })?;
            f(env, &context)
        })
    }
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple {
    use super::{
        ApplePictureInPictureRenderFrame, ApplePictureInPictureSetExternalRendering,
        PictureInPictureCommand, PictureInPictureControllerState, PictureInPictureHostId,
    };
    use core::ffi::c_void;
    use std::time::Duration;
    use waterkit_video_core::Error as VideoError;

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
    const COMMAND_ACTIVE: i32 = 5;
    const COMMAND_INACTIVE: i32 = 6;

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
        fn waterkit_video_apple_pip_bridge_open_command_channel(host_id: u64);
        fn waterkit_video_apple_pip_bridge_close_command_channel(host_id: u64);
        fn waterkit_video_apple_pip_bridge_wait_command_kind(
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

    pub(super) fn open_picture_in_picture_command_channel(host_id: PictureInPictureHostId) {
        unsafe { waterkit_video_apple_pip_bridge_open_command_channel(host_id.get()) }
    }

    pub(super) fn close_picture_in_picture_command_channel(host_id: PictureInPictureHostId) {
        unsafe { waterkit_video_apple_pip_bridge_close_command_channel(host_id.get()) }
    }

    pub(super) fn wait_picture_in_picture_command(
        host_id: PictureInPictureHostId,
    ) -> Result<Option<PictureInPictureCommand>, VideoError> {
        let mut kind = COMMAND_NONE;
        let mut value_secs = 0.0;
        unsafe {
            waterkit_video_apple_pip_bridge_wait_command_kind(
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
            COMMAND_ACTIVE => Some(PictureInPictureCommand::ActiveChanged(true)),
            COMMAND_INACTIVE => Some(PictureInPictureCommand::ActiveChanged(false)),
            _ => {
                return Err(VideoError::Unsupported(format!(
                    "Apple picture in picture helper returned unknown command kind {kind}"
                )));
            }
        };
        Ok(command)
    }
}

#[cfg(all(test, any(target_os = "ios", target_os = "macos")))]
mod tests {
    use core::num::NonZeroU64;

    use super::{PictureInPictureCommandStream, PictureInPictureHostId};

    #[test]
    fn command_stream_shutdown_wakes_blocked_platform_waiter() {
        let host_id = PictureInPictureHostId::new(
            NonZeroU64::new(1).expect("test picture-in-picture host id must be non-zero"),
        );
        drop(PictureInPictureCommandStream::new(host_id));
    }
}
