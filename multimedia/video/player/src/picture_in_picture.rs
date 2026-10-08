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
/// code receives commands from the async channel without polling on each
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
pub struct PictureInPictureController {
    host_id: PictureInPictureHostId,
}

impl PictureInPictureController {
    /// Creates a controller for one stable render host.
    #[must_use]
    pub const fn new(host_id: PictureInPictureHostId) -> Self {
        Self { host_id }
    }

    /// Requests picture in picture using the displayed video aspect ratio.
    ///
    /// # Errors
    ///
    /// Returns an error when the platform does not support picture in picture
    /// or the host is not registered/configured for it.
    #[cfg_attr(
        not(any(target_os = "ios", target_os = "macos")),
        expect(
            clippy::unused_async,
            reason = "the Apple path awaits a main-thread hop; other backends run synchronously"
        )
    )]
    pub async fn enter(&mut self, aspect_ratio: Option<(u32, u32)>) -> Result<(), VideoError> {
        #[cfg(target_os = "android")]
        {
            android::enter(aspect_ratio)
        }

        #[cfg(any(target_os = "ios", target_os = "macos"))]
        {
            apple::enter_picture_in_picture(self.host_id, aspect_ratio).await
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
    /// On a platform with no picture-in-picture concept there is no controller
    /// to inform, so the sync completes vacuously. Requesting picture in
    /// picture is where the missing capability surfaces: [`Self::enter`]
    /// returns [`VideoError::Unsupported`] there.
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
            android::sync(state)
        }

        #[cfg(any(target_os = "ios", target_os = "macos"))]
        {
            let mtm = objc2::MainThreadMarker::new()
                .expect("PictureInPictureController::sync must be called on the main thread");
            apple::sync_picture_in_picture_controller(state, mtm);
            Ok(())
        }

        #[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
        {
            Ok(())
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
            android::is_active()
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
    let mtm = objc2::MainThreadMarker::new()
        .expect("waterkit_video_apple_register_gpu_surface_host must be called on the main thread");
    apple::register_gpu_surface_host(
        host_id,
        user_data,
        render_frame,
        set_external_rendering,
        mtm,
    );
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
    let mtm = objc2::MainThreadMarker::new().expect(
        "waterkit_video_apple_unregister_gpu_surface_host must be called on the main thread",
    );
    apple::unregister_gpu_surface_host(host_id, mtm);
}

#[cfg(target_os = "android")]
mod android {
    use super::PictureInPictureControllerState;
    use crate::android_surface::with_attached_env;
    use jni::{
        Env, JavaVM, jni_sig, jni_str,
        objects::{JObject, JValue},
    };
    use std::convert::TryFrom;
    use waterkit_build::{DexHelper, dex_helper};
    use waterkit_video_core::Error as VideoError;

    /// `waterkit.video.PictureInPictureHelper`, compiled into the app's DEX by
    /// the packager and resolved through the application's `ClassLoader`.
    static HELPER: DexHelper = dex_helper!("waterkit.video.PictureInPictureHelper");

    const RESULT_ENTERED: i32 = 0;
    const RESULT_PLATFORM_UNSUPPORTED: i32 = 1;
    const RESULT_DEVICE_UNSUPPORTED: i32 = 2;
    const RESULT_ACTIVITY_UNAVAILABLE: i32 = 3;
    const RESULT_ACTIVITY_NOT_DECLARED: i32 = 4;
    const RESULT_ENTER_FAILED: i32 = 5;

    pub(super) fn sync(state: PictureInPictureControllerState) -> Result<(), VideoError> {
        with_android_context(|env, context| {
            let helper_class = helper_class(env, context)?;
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

    pub(super) fn enter(aspect_ratio: Option<(u32, u32)>) -> Result<(), VideoError> {
        with_android_context(|env, context| {
            let helper_class = helper_class(env, context)?;
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

    pub(super) fn is_active() -> Result<bool, VideoError> {
        with_android_context(|env, context| {
            let helper_class = helper_class(env, context)?;
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

    fn helper_class(
        env: &mut Env<'_>,
        context: &JObject<'_>,
    ) -> Result<&'static jni::objects::Global<jni::objects::JClass<'static>>, VideoError> {
        HELPER.class(env, context).map_err(|error| {
            VideoError::Unsupported(format!(
                "Android picture in picture helper class unavailable: {error}"
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
    use core::cell::{Cell, OnceCell, RefCell};
    use core::ffi::c_void;
    use core::ptr::NonNull;

    use std::collections::{HashMap, VecDeque};
    use std::rc::Rc;
    use std::sync::{Arc, Condvar, LazyLock, Mutex, OnceLock};
    use std::time::Duration;
    use waterkit_video_core::Error as VideoError;

    use block2::{Block, RcBlock};
    use dispatch2::MainThreadBound;
    use objc2::available;
    use objc2::rc::{Retained, Weak};
    use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{
        ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    };
    use objc2_av_foundation::{
        AVLayerVideoGravityResizeAspect, AVQueuedSampleBufferRendering,
        AVQueuedSampleBufferRenderingStatus, AVSampleBufferDisplayLayer,
        AVSampleBufferVideoRenderer,
    };
    use objc2_av_kit::{
        AVPictureInPictureController, AVPictureInPictureControllerContentSource,
        AVPictureInPictureControllerDelegate, AVPictureInPictureSampleBufferPlaybackDelegate,
    };
    use objc2_core_foundation::{
        CFBoolean, CFDictionary, CFMutableDictionary, CFNumber, CFRetained, CFString, CFType,
    };
    use objc2_core_media::{
        CMSampleBuffer, CMSampleTimingInfo, CMTime, CMTimeRange, CMVideoDimensions,
        CMVideoFormatDescription, CMVideoFormatDescriptionCreateForImageBuffer,
        kCMSampleAttachmentKey_DisplayImmediately, kCMTimeInvalid, kCMTimePositiveInfinity,
        kCMTimeRangeInvalid, kCMTimeZero,
    };
    use objc2_core_video::{
        CVMetalTexture, CVMetalTextureCache, CVMetalTextureGetTexture, CVPixelBuffer,
        CVPixelBufferGetHeight, CVPixelBufferGetWidth, kCVPixelBufferHeightKey,
        kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey,
        kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey, kCVPixelFormatType_32BGRA,
        kCVReturnSuccess,
    };
    use objc2_foundation::{NSError, NSRunLoop, NSRunLoopCommonModes, NSTimer};
    use objc2_metal::{MTLCreateSystemDefaultDevice, MTLDevice, MTLPixelFormat, MTLTexture};

    const PICTURE_IN_PICTURE_FRAME_INTERVAL: f64 = 1.0 / 30.0;
    const DEFAULT_RENDER_WIDTH: i32 = 960;
    const DEFAULT_RENDER_HEIGHT: i32 = 540;

    /// `enter` outcomes, matching the codes the bridge helper returned.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum EnterResult {
        Success,
        Unsupported,
        HostNotRegistered,
        NotPossible,
        StartFailed,
    }

    struct CommandChannelState {
        commands: VecDeque<PictureInPictureCommand>,
        closed: bool,
    }

    /// Blocking command queue one `PiP` host drains on its worker thread; `close`
    /// wakes the waiter, mirroring the `NSCondition` channel the bridge used.
    struct CommandChannel {
        state: Mutex<CommandChannelState>,
        condition: Condvar,
    }

    impl CommandChannel {
        #[expect(
            clippy::missing_const_for_fn,
            reason = "Mutex::new over a VecDeque field is not const-compatible here"
        )]
        fn new() -> Self {
            Self {
                state: Mutex::new(CommandChannelState {
                    commands: VecDeque::new(),
                    closed: false,
                }),
                condition: Condvar::new(),
            }
        }

        fn send(&self, command: PictureInPictureCommand) {
            {
                let mut state = self.state.lock().unwrap();
                if state.closed {
                    return;
                }
                state.commands.push_back(command);
            }
            self.condition.notify_one();
        }

        fn wait(&self) -> Option<PictureInPictureCommand> {
            let mut state = self.state.lock().unwrap();
            loop {
                if let Some(command) = state.commands.pop_front() {
                    return Some(command);
                }
                if state.closed {
                    return None;
                }
                state = self.condition.wait(state).unwrap();
            }
        }

        fn close(&self) {
            {
                let mut state = self.state.lock().unwrap();
                state.closed = true;
                state.commands.clear();
            }
            self.condition.notify_all();
        }
    }

    static COMMAND_CHANNELS: LazyLock<Mutex<HashMap<u64, Arc<CommandChannel>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    fn open_command_channel(host_id: u64) -> Arc<CommandChannel> {
        let mut channels = COMMAND_CHANNELS.lock().unwrap();
        Arc::clone(
            channels
                .entry(host_id)
                .or_insert_with(|| Arc::new(CommandChannel::new())),
        )
    }

    fn close_command_channel(host_id: u64) {
        let channel = COMMAND_CHANNELS.lock().unwrap().remove(&host_id);
        if let Some(channel) = channel {
            channel.close();
        }
    }

    /// One registered render host: the callback pair and the metadata the
    /// manager mutates on the main thread.
    struct HostRegistration {
        host_id: u64,
        user_data: *mut c_void,
        render_frame: Cell<ApplePictureInPictureRenderFrame>,
        set_external_rendering: Cell<ApplePictureInPictureSetExternalRendering>,
        active: Cell<bool>,
        playing: Cell<bool>,
        aspect_ratio: Cell<Option<(u32, u32)>>,
        command_channel: Arc<CommandChannel>,
    }

    /// Pointer helpers for `CFMutableDictionary::set_value`, which takes
    /// object addresses as untyped `*const c_void`.
    const fn cf_dict_key(key: &CFString) -> *const c_void {
        std::ptr::from_ref(key).cast()
    }

    fn cf_dict_value<T: ?Sized + objc2_core_foundation::Type>(
        value: &CFRetained<T>,
    ) -> *const c_void {
        CFRetained::as_ptr(value).as_ptr().cast()
    }

    const fn cf_dict_value_ref<T: ?Sized>(value: &T) -> *const c_void {
        std::ptr::from_ref(value).cast()
    }

    /// Registered hosts that never called register (a bare `sync` arrives
    /// first) get no-op callback pairs until a real registration lands.
    const unsafe extern "C" fn noop_render_frame(
        _: *mut c_void,
        _: *mut c_void,
        _: u32,
        _: u32,
    ) -> bool {
        false
    }

    const unsafe extern "C" fn noop_set_external_rendering(_: *mut c_void, _: bool) {}

    /// The shared manager; every use happens on the main thread.
    struct PictureInPictureManager {
        hosts: HashMap<u64, Rc<HostRegistration>>,
        active_session: Option<Retained<PictureInPictureSession>>,
    }

    static MANAGER: OnceLock<MainThreadBound<RefCell<PictureInPictureManager>>> = OnceLock::new();

    /// Fetches the manager; creates it lazily on the caller's first main-thread
    /// visit. Must be called on the main thread.
    fn manager(mtm: MainThreadMarker) -> &'static RefCell<PictureInPictureManager> {
        MANAGER
            .get_or_init(|| {
                MainThreadBound::new(
                    RefCell::new(PictureInPictureManager {
                        hosts: HashMap::new(),
                        active_session: None,
                    }),
                    mtm,
                )
            })
            .get(mtm)
    }

    fn pip_available() -> bool {
        available!(ios = 15.0, macos = 12.0, ..)
    }

    /// Render target shared by the frame pump: pixel buffer + Metal texture +
    /// format description rebuilt when the render size changes.
    struct RenderTarget {
        pixel_buffer: CFRetained<CVPixelBuffer>,
        metal_texture: Retained<ProtocolObject<dyn MTLTexture>>,
        format_description: CFRetained<CMVideoFormatDescription>,
    }

    pub struct PictureInPictureSessionIvars {
        host: Rc<HostRegistration>,
        _device: Retained<ProtocolObject<dyn MTLDevice>>,
        texture_cache: CFRetained<CVMetalTextureCache>,
        display_layer: Retained<AVSampleBufferDisplayLayer>,
        renderer: Retained<AVSampleBufferVideoRenderer>,
        controller: OnceCell<Retained<AVPictureInPictureController>>,
        timer: RefCell<Option<Retained<NSTimer>>>,
        render_size: Cell<CMVideoDimensions>,
        render_target: RefCell<Option<RenderTarget>>,
        frame_index: Cell<i64>,
    }

    define_class!(
        // SAFETY:
        // - The superclass NSObject does not have any subclassing requirements.
        // - `PictureInPictureSession` does not implement `Drop`.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WaterkitPictureInPictureSession"]
        #[ivars = PictureInPictureSessionIvars]
        struct PictureInPictureSession;

        unsafe impl NSObjectProtocol for PictureInPictureSession {}

        unsafe impl AVPictureInPictureControllerDelegate for PictureInPictureSession {
            #[unsafe(method(pictureInPictureControllerWillStartPictureInPicture:))]
            fn will_start(&self, _controller: &AVPictureInPictureController) {
                self.start_frame_pump();
            }

            #[unsafe(method(pictureInPictureControllerDidStartPictureInPicture:))]
            fn did_start(&self, _controller: &AVPictureInPictureController) {
                self.ivars()
                    .host
                    .command_channel
                    .send(PictureInPictureCommand::ActiveChanged(true));
            }

            #[unsafe(method(pictureInPictureControllerWillStopPictureInPicture:))]
            fn will_stop(&self, _controller: &AVPictureInPictureController) {
                self.stop_frame_pump();
            }

            #[unsafe(method(pictureInPictureControllerDidStopPictureInPicture:))]
            fn did_stop(&self, _controller: &AVPictureInPictureController) {
                let host_id = self.ivars().host.host_id;
                self.ivars()
                    .host
                    .command_channel
                    .send(PictureInPictureCommand::ActiveChanged(false));
                let mtm = MainThreadMarker::new()
                    .expect("PiP controller delegates run on the main thread");
                manager(mtm).borrow_mut().session_did_stop(host_id);
            }

            #[unsafe(method(pictureInPictureController:failedToStartPictureInPictureWithError:))]
            fn failed_to_start(&self, _controller: &AVPictureInPictureController, error: &NSError) {
                tracing::error!(
                    host_id = self.ivars().host.host_id,
                    error = %error.localizedDescription(),
                    "picture in picture failed to start"
                );
                let host_id = self.ivars().host.host_id;
                self.ivars()
                    .host
                    .command_channel
                    .send(PictureInPictureCommand::ActiveChanged(false));
                let mtm = MainThreadMarker::new()
                    .expect("PiP controller delegates run on the main thread");
                manager(mtm).borrow_mut().session_did_stop(host_id);
            }
        }

        unsafe impl AVPictureInPictureSampleBufferPlaybackDelegate for PictureInPictureSession {
            #[unsafe(method(pictureInPictureController:setPlaying:))]
            fn set_playing(&self, _controller: &AVPictureInPictureController, playing: bool) {
                self.ivars().host.command_channel.send(if playing {
                    PictureInPictureCommand::Play
                } else {
                    PictureInPictureCommand::Pause
                });
            }

            #[unsafe(method(pictureInPictureControllerTimeRangeForPlayback:))]
            fn time_range_for_playback(
                &self,
                _controller: &AVPictureInPictureController,
            ) -> CMTimeRange {
                if !self.ivars().host.active.get() {
                    // SAFETY: the invalid sentinel is a documented constant.
                    return unsafe { kCMTimeRangeInvalid };
                }
                // SAFETY: zero start and positive-infinite duration are
                // documented CMTime constants.
                unsafe { CMTimeRange::new(kCMTimeZero, kCMTimePositiveInfinity) }
            }

            #[unsafe(method(pictureInPictureControllerIsPlaybackPaused:))]
            fn is_playback_paused(&self, _controller: &AVPictureInPictureController) -> bool {
                !self.ivars().host.playing.get()
            }

            #[unsafe(method(pictureInPictureController:didTransitionToRenderSize:))]
            fn did_transition_to_render_size(
                &self,
                _controller: &AVPictureInPictureController,
                new_render_size: CMVideoDimensions,
            ) {
                if new_render_size.width <= 0 || new_render_size.height <= 0 {
                    return;
                }
                self.ivars().render_size.set(new_render_size);
                self.ensure_render_target(new_render_size);
            }

            #[unsafe(method(pictureInPictureController:skipByInterval:completionHandler:))]
            fn skip_by_interval(
                &self,
                _controller: &AVPictureInPictureController,
                skip_interval: CMTime,
                completion_handler: &Block<dyn Fn()>,
            ) {
                let seconds = unsafe { skip_interval.seconds() };
                if seconds > 0.0 {
                    self.ivars()
                        .host
                        .command_channel
                        .send(PictureInPictureCommand::SeekForward(
                            Duration::from_secs_f64(seconds),
                        ));
                } else if seconds < 0.0 {
                    self.ivars()
                        .host
                        .command_channel
                        .send(PictureInPictureCommand::SeekBackward(
                            Duration::from_secs_f64(-seconds),
                        ));
                }
                completion_handler.call(());
            }

            #[unsafe(method(pictureInPictureControllerShouldProhibitBackgroundAudioPlayback:))]
            fn should_prohibit_background_audio(
                &self,
                _controller: &AVPictureInPictureController,
            ) -> bool {
                false
            }
        }
    );

    impl Drop for PictureInPictureSession {
        fn drop(&mut self) {
            if let Some(timer) = self.ivars().timer.borrow_mut().take() {
                timer.invalidate();
            }
            let host = &self.ivars().host;
            unsafe {
                host.set_external_rendering.get()(host.user_data, false);
            }
        }
    }

    impl PictureInPictureSession {
        /// Builds a session on the main thread: device + texture cache +
        /// display layer + controller, matching the bridge `init`.
        fn new(host: Rc<HostRegistration>, mtm: MainThreadMarker) -> Option<Retained<Self>> {
            // SAFETY: `MTLCreateSystemDefaultDevice` returns the default GPU
            // device or nil when Metal is unavailable.
            let device = MTLCreateSystemDefaultDevice()?;
            let mut cache = std::ptr::null_mut::<CVMetalTextureCache>();
            // SAFETY: `cache` is a valid out-pointer for the duration of the call.
            let status = unsafe {
                CVMetalTextureCache::create(None, None, &device, None, NonNull::from(&mut cache))
            };
            // SAFETY: a successful create returned a +1 object in `cache`.
            let texture_cache = if status == kCVReturnSuccess && !cache.is_null() {
                unsafe { CFRetained::from_raw(NonNull::new(cache).unwrap()) }
            } else {
                tracing::error!(status, "CVMetalTextureCacheCreate failed");
                return None;
            };
            // SAFETY: `new` is `[[AVSampleBufferDisplayLayer alloc] init]` on
            // the main thread.
            let display_layer = unsafe { AVSampleBufferDisplayLayer::new() };
            // SAFETY: documented CALayer/AVSampleBufferDisplayLayer setters.
            unsafe {
                display_layer.setVideoGravity(
                    AVLayerVideoGravityResizeAspect
                        .expect("AVLayerVideoGravityResizeAspect is present"),
                );
                display_layer.setOpaque(true);
            }
            // SAFETY: `sampleBufferRenderer` is the thread-safe enqueue path
            // the deprecated layer methods point at.
            let renderer = unsafe { display_layer.sampleBufferRenderer() };

            let render_size = Cell::new(Self::initial_render_size(&host));
            let this = Self::alloc(mtm).set_ivars(PictureInPictureSessionIvars {
                host,
                _device: device,
                texture_cache,
                display_layer,
                renderer,
                controller: OnceCell::new(),
                timer: RefCell::new(None),
                render_size,
                render_target: RefCell::new(None),
                frame_index: Cell::new(0),
            });
            // SAFETY: `this` is freshly allocated; `init` has no extra requirements.
            let this: Retained<Self> = unsafe { msg_send![super(this), init] };

            let display_layer = this.ivars().display_layer.clone();
            // SAFETY: `initWithSampleBufferDisplayLayer:playbackDelegate:` is
            // the documented content-source initializer; the delegate object
            // outlives the source because the manager owns the session.
            let source = unsafe {
                AVPictureInPictureControllerContentSource::initWithSampleBufferDisplayLayer_playbackDelegate(
                    msg_send![AVPictureInPictureControllerContentSource::class(), alloc],
                    &display_layer,
                    ProtocolObject::from_ref(&*this),
                )
            };
            // SAFETY: `initWithContentSource:` is the documented initializer.
            let controller = unsafe {
                AVPictureInPictureController::initWithContentSource(
                    msg_send![AVPictureInPictureController::class(), alloc],
                    &source,
                )
            };
            // SAFETY: `delegate` is a weak property; the manager's
            // `active_session` retain keeps the session alive.
            unsafe { controller.setDelegate(Some(ProtocolObject::from_ref(&*this))) };
            this.ivars()
                .controller
                .set(controller)
                .expect("controller is set exactly once at init");

            let host_ref = &this.ivars().host;
            // SAFETY: `setExternalRendering` is the registered callback pair's
            // toggle; `user_data` belongs to the registered host.
            unsafe {
                host_ref.set_external_rendering.get()(host_ref.user_data, true);
            }
            // SAFETY: `requiresLinearPlayback` is a documented property.
            unsafe {
                this.ivars()
                    .controller
                    .get()
                    .expect("controller initialized above")
                    .setRequiresLinearPlayback(false);
            }
            let size = this.ivars().render_size.get();
            this.ensure_render_target(size);
            this.enqueue_frame();
            // SAFETY: `invalidatePlaybackState` refreshes the PiP controls.
            unsafe {
                this.ivars()
                    .controller
                    .get()
                    .expect("controller initialized above")
                    .invalidatePlaybackState();
            };
            Some(this)
        }

        fn controller(&self) -> &AVPictureInPictureController {
            self.ivars()
                .controller
                .get()
                .expect("controller exists after init")
        }

        fn start(&self) -> EnterResult {
            // SAFETY: class query for PiP availability.
            if !unsafe { AVPictureInPictureController::isPictureInPictureSupported() } {
                return EnterResult::Unsupported;
            }
            if !self.ivars().host.active.get() {
                return EnterResult::NotPossible;
            }
            if !self.ensure_render_target(self.ivars().render_size.get()) {
                return EnterResult::StartFailed;
            }

            self.enqueue_frame();
            self.start_frame_pump();
            // SAFETY: documented controller calls on the main thread.
            unsafe { self.controller().invalidatePlaybackState() };

            // SAFETY: documented controller calls on the main thread.
            if !unsafe { self.controller().isPictureInPicturePossible() } {
                tracing::error!(
                    host_id = self.ivars().host.host_id,
                    "picture in picture is not possible for host"
                );
                self.stop_frame_pump();
                let host = &self.ivars().host;
                unsafe {
                    host.set_external_rendering.get()(host.user_data, false);
                }
                return EnterResult::NotPossible;
            }

            // SAFETY: starts picture in picture; delegate callbacks report
            // failure through `failedToStart`.
            unsafe { self.controller().startPictureInPicture() };
            EnterResult::Success
        }

        fn update_from_host_state(&self) {
            if self.ivars().host.aspect_ratio.get().is_some() {
                let next = Self::initial_render_size_for(self.ivars().host.aspect_ratio.get());
                let current = self.ivars().render_size.get();
                if next.width != current.width || next.height != current.height {
                    self.ivars().render_size.set(next);
                    self.ensure_render_target(next);
                }
            }

            // SAFETY: documented controller calls on the main thread.
            unsafe {
                if !self.ivars().host.active.get() && self.controller().isPictureInPictureActive() {
                    self.controller().stopPictureInPicture();
                }
                self.controller().invalidatePlaybackState();
            }
        }

        fn is_active(&self) -> bool {
            // SAFETY: documented controller calls on the main thread.
            unsafe { self.controller().isPictureInPictureActive() }
        }

        fn start_frame_pump(&self) {
            if self.ivars().timer.borrow().is_some() {
                return;
            }
            let session = Weak::new(self);
            let block = RcBlock::new(move |_timer| {
                if let Some(session) = session.load() {
                    session.enqueue_frame();
                }
            });
            // SAFETY: `timerWithTimeInterval:repeats:block:` with a retained
            // block; added to the main run loop below.
            let timer = unsafe {
                NSTimer::timerWithTimeInterval_repeats_block(
                    PICTURE_IN_PICTURE_FRAME_INTERVAL,
                    true,
                    &block,
                )
            };
            // SAFETY: adding a valid timer to the main run loop's common modes.
            unsafe {
                NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
            }
            self.ivars().timer.borrow_mut().replace(timer);
        }

        fn stop_frame_pump(&self) {
            if let Some(timer) = self.ivars().timer.borrow_mut().take() {
                timer.invalidate();
            }
        }

        fn enqueue_frame(&self) {
            if !self.ivars().host.active.get() {
                return;
            }
            let size = self.ivars().render_size.get();
            if !self.ensure_render_target(size) {
                return;
            }
            let target = self.ivars().render_target.borrow();
            let Some(target) = target.as_ref() else {
                return;
            };
            let host = &self.ivars().host;
            // SAFETY: `render_frame` writes into the live Metal texture;
            // `as_ptr` keeps it borrowed for the callback's duration.
            let rendered = unsafe {
                host.render_frame.get()(
                    host.user_data,
                    Retained::as_ptr(&target.metal_texture).cast_mut().cast(),
                    size.width.cast_unsigned(),
                    size.height.cast_unsigned(),
                )
            };
            if !rendered {
                tracing::debug!(
                    host_id = host.host_id,
                    "render callback returned false for picture-in-picture host"
                );
                return;
            }

            let video_renderer = &self.ivars().renderer;
            // SAFETY: `status` is a documented getter.
            if unsafe { video_renderer.status() } == AVQueuedSampleBufferRenderingStatus::Failed {
                tracing::warn!(
                    host_id = host.host_id,
                    "sample buffer display layer failed, flushing"
                );
                // SAFETY: mirrors `flushAndRemoveImage`; the handler
                // fires when the remove completes, which the bridge did
                // not wait on either.
                unsafe {
                    video_renderer.flushWithRemovalOfDisplayedImage_completionHandler(
                        true,
                        Some(&block2::RcBlock::new(|| {})),
                    );
                }
            }

            let Some(sample_buffer) = self.make_sample_buffer(&target.pixel_buffer) else {
                tracing::error!(
                    host_id = host.host_id,
                    "failed to create sample buffer for picture-in-picture host"
                );
                return;
            };
            // SAFETY: enqueues a live sample buffer.
            unsafe {
                <AVSampleBufferVideoRenderer as AVQueuedSampleBufferRendering>::enqueueSampleBuffer(
                    video_renderer,
                    &sample_buffer,
                );
            }
            self.ivars()
                .frame_index
                .set(self.ivars().frame_index.get().wrapping_add(1));
        }

        fn ensure_render_target(&self, size: CMVideoDimensions) -> bool {
            {
                let target = self.ivars().render_target.borrow();
                if let Some(target) = target.as_ref() {
                    // SAFETY: documented size getters on a live pixel buffer.
                    let (width, height) = (
                        CVPixelBufferGetWidth(&target.pixel_buffer),
                        CVPixelBufferGetHeight(&target.pixel_buffer),
                    );
                    if width == usize::try_from(size.width).unwrap_or_default()
                        && height == usize::try_from(size.height).unwrap_or_default()
                    {
                        return true;
                    }
                }
            }

            let Some(render_target) = self.new_render_target(size) else {
                return false;
            };
            *self.ivars().render_target.borrow_mut() = Some(render_target);
            true
        }

        /// Rebuilds the pixel-buffer / texture / format-description triple at
        /// `size`; fails the same way the bridge `ensureRenderTarget` did.
        /// Builds the attribute dictionary `CVPixelBufferCreate` expects:
        /// 32-BGRA pixels at `size`, Metal-compatible, IOSurface-backed.
        fn pixel_buffer_attributes(
            size: CMVideoDimensions,
        ) -> CFRetained<CFMutableDictionary<CFString, CFType>> {
            let attributes = CFMutableDictionary::<CFString, CFType>::empty();
            let pixel_format = CFNumber::new_cgfloat(kCVPixelFormatType_32BGRA.into());
            let buffer_width = CFNumber::new_cgfloat(size.width.into());
            let buffer_height = CFNumber::new_cgfloat(size.height.into());
            let iosurface_properties = CFDictionary::<CFString, CFBoolean>::empty();
            let opaque = attributes.as_opaque();
            let entries: [(*const c_void, *const c_void); 5] = [
                (
                    cf_dict_key(unsafe { kCVPixelBufferPixelFormatTypeKey }),
                    cf_dict_value(&pixel_format),
                ),
                (
                    cf_dict_key(unsafe { kCVPixelBufferWidthKey }),
                    cf_dict_value(&buffer_width),
                ),
                (
                    cf_dict_key(unsafe { kCVPixelBufferHeightKey }),
                    cf_dict_value(&buffer_height),
                ),
                (
                    cf_dict_key(unsafe { kCVPixelBufferMetalCompatibilityKey }),
                    cf_dict_value_ref(CFBoolean::new(true)),
                ),
                (
                    cf_dict_key(unsafe { kCVPixelBufferIOSurfacePropertiesKey }),
                    cf_dict_value(&iosurface_properties),
                ),
            ];
            for (key, value) in entries {
                // SAFETY: `key`/`value` point at live CF objects; the
                // dictionary retains the value per its default callbacks.
                unsafe { CFMutableDictionary::set_value(Some(opaque), key, value) };
            }
            attributes
        }

        fn new_render_target(&self, size: CMVideoDimensions) -> Option<RenderTarget> {
            let attributes = Self::pixel_buffer_attributes(size);
            let mut pixel_buffer = std::ptr::null_mut::<CVPixelBuffer>();
            // SAFETY: `pixel_buffer` is a valid out-pointer; `attributes`
            // outlives the call.
            let status = unsafe {
                objc2_core_video::CVPixelBufferCreate(
                    None,
                    usize::try_from(size.width).unwrap_or_default(),
                    usize::try_from(size.height).unwrap_or_default(),
                    kCVPixelFormatType_32BGRA,
                    Some(&*(CFRetained::as_ptr(&attributes).as_ptr() as *const CFDictionary)),
                    NonNull::from(&mut pixel_buffer),
                )
            };
            let pixel_buffer = if status == kCVReturnSuccess && !pixel_buffer.is_null() {
                // SAFETY: a successful create returned a +1 object.
                Some(unsafe { CFRetained::from_raw(NonNull::new(pixel_buffer).unwrap()) })
            } else {
                None
            };
            let Some(pixel_buffer) = pixel_buffer else {
                tracing::error!(status, "CVPixelBufferCreate failed");
                return None;
            };

            let mut cv_texture = std::ptr::null_mut::<CVMetalTexture>();
            // SAFETY: `cv_texture` is a valid out-pointer.
            let texture_status = unsafe {
                CVMetalTextureCache::create_texture_from_image(
                    None,
                    &self.ivars().texture_cache,
                    &pixel_buffer,
                    None,
                    MTLPixelFormat::BGRA8Unorm,
                    usize::try_from(size.width).unwrap_or_default(),
                    usize::try_from(size.height).unwrap_or_default(),
                    0,
                    NonNull::from(&mut cv_texture),
                )
            };
            let metal_texture = if texture_status == kCVReturnSuccess && !cv_texture.is_null() {
                // SAFETY: a successful create returned a +1 texture ref;
                // `CVMetalTextureGetTexture` yields the backing object.
                let cv_texture = unsafe { CFRetained::from_raw(NonNull::new(cv_texture).unwrap()) };
                // `CVMetalTextureGetTexture` retains the +0 texture the
                // image points at.
                CVMetalTextureGetTexture(&cv_texture)
            } else {
                None
            };
            let Some(metal_texture) = metal_texture else {
                tracing::error!(
                    texture_status,
                    "CVMetalTextureCacheCreateTextureFromImage failed"
                );
                return None;
            };

            let mut format_description = std::ptr::null::<CMVideoFormatDescription>();
            // SAFETY: `format_description` is a valid out-pointer.
            let format_status = unsafe {
                CMVideoFormatDescriptionCreateForImageBuffer(
                    None,
                    &pixel_buffer,
                    NonNull::from(&mut format_description),
                )
            };
            let format_description = if format_status == 0 && !format_description.is_null() {
                // SAFETY: a successful create returned a +1 object.
                Some(unsafe {
                    CFRetained::from_raw(NonNull::new(format_description.cast_mut()).unwrap())
                })
            } else {
                None
            };
            let Some(format_description) = format_description else {
                tracing::error!(
                    format_status,
                    "CMVideoFormatDescriptionCreateForImageBuffer failed"
                );
                return None;
            };

            Some(RenderTarget {
                pixel_buffer,
                metal_texture,
                format_description,
            })
        }

        fn make_sample_buffer(
            &self,
            pixel_buffer: &CVPixelBuffer,
        ) -> Option<CFRetained<CMSampleBuffer>> {
            const TIME_SCALE: i32 = 600;

            let target = self.ivars().render_target.borrow();
            let format_description = &target
                .as_ref()
                .expect("render target exists")
                .format_description;
            // SAFETY: `CMTimeMake` is the documented constructor.
            let presentation_time =
                unsafe { CMTime::new(self.ivars().frame_index.get(), TIME_SCALE) };
            let mut timing = CMSampleTimingInfo {
                duration: unsafe { CMTime::new(1, TIME_SCALE) },
                presentationTimeStamp: presentation_time,
                decodeTimeStamp: unsafe { kCMTimeInvalid },
            };

            let mut sample_buffer = std::ptr::null_mut::<CMSampleBuffer>();
            // SAFETY: `timing`/`sample_buffer` are valid in/out-pointers.
            let status = unsafe {
                CMSampleBuffer::create_ready_with_image_buffer(
                    None,
                    pixel_buffer,
                    format_description,
                    NonNull::from(&mut timing),
                    NonNull::from(&mut sample_buffer),
                )
            };
            if status != 0 || sample_buffer.is_null() {
                tracing::error!(status, "CMSampleBufferCreateReadyWithImageBuffer failed");
                return None;
            }
            // SAFETY: a successful create returned a +1 object.
            let sample_buffer =
                unsafe { CFRetained::from_raw(NonNull::new(sample_buffer).unwrap()) };

            // SAFETY: asks CoreMedia to create the attachment array if absent.
            if let Some(attachments) = unsafe { sample_buffer.sample_attachments_array(true) } {
                // SAFETY: index 0 exists — `create` filled the array when
                // CoreMedia lacked one, and sample buffers always carry one
                // attachment dictionary per sample.
                let attachment = unsafe { attachments.value_at_index(0) };
                // SAFETY: `attachment` is the CFMutableDictionary the array
                // exposes; setting the documented display-immediately key.
                unsafe {
                    CFMutableDictionary::set_value(
                        Some(&*attachment.cast::<CFMutableDictionary>()),
                        cf_dict_key(kCMSampleAttachmentKey_DisplayImmediately),
                        cf_dict_value_ref(CFBoolean::new(true)),
                    );
                }
            }

            Some(sample_buffer)
        }

        fn initial_render_size(host: &HostRegistration) -> CMVideoDimensions {
            Self::initial_render_size_for(host.aspect_ratio.get())
        }

        #[expect(
            clippy::cast_possible_truncation,
            reason = "aspect-derived height stays far below i32::MAX for sane inputs"
        )]
        fn initial_render_size_for(aspect_ratio: Option<(u32, u32)>) -> CMVideoDimensions {
            let Some((width, height)) = aspect_ratio else {
                return CMVideoDimensions {
                    width: DEFAULT_RENDER_WIDTH,
                    height: DEFAULT_RENDER_HEIGHT,
                };
            };
            if width == 0 || height == 0 {
                return CMVideoDimensions {
                    width: DEFAULT_RENDER_WIDTH,
                    height: DEFAULT_RENDER_HEIGHT,
                };
            }
            let height = (f64::from(DEFAULT_RENDER_WIDTH) * f64::from(height) / f64::from(width))
                .round()
                .max(1.0) as i32;
            CMVideoDimensions {
                width: DEFAULT_RENDER_WIDTH,
                height,
            }
        }
    }

    impl PictureInPictureManager {
        fn register_host(
            &mut self,
            host_id: u64,
            user_data: *mut c_void,
            render_frame: ApplePictureInPictureRenderFrame,
            set_external_rendering: ApplePictureInPictureSetExternalRendering,
        ) {
            if let Some(host) = self.hosts.get(&host_id) {
                host.render_frame.set(render_frame);
                host.set_external_rendering.set(set_external_rendering);
                return;
            }
            self.hosts.insert(
                host_id,
                Rc::new(HostRegistration {
                    host_id,
                    user_data,
                    render_frame: Cell::new(render_frame),
                    set_external_rendering: Cell::new(set_external_rendering),
                    active: Cell::new(false),
                    playing: Cell::new(false),
                    aspect_ratio: Cell::new(None),
                    command_channel: open_command_channel(host_id),
                }),
            );
        }

        fn unregister_host(&mut self, host_id: u64) {
            if let Some(session) = &self.active_session
                && session.ivars().host.host_id == host_id
            {
                self.active_session = None;
            }
            self.hosts.remove(&host_id);
            close_command_channel(host_id);
        }

        fn sync_host_state(
            &mut self,
            host_id: u64,
            active: bool,
            playing: bool,
            aspect_ratio: Option<(u32, u32)>,
        ) {
            let host = self.hosts.entry(host_id).or_insert_with(|| {
                Rc::new(HostRegistration {
                    host_id,
                    user_data: std::ptr::null_mut(),
                    render_frame: Cell::new(noop_render_frame),
                    set_external_rendering: Cell::new(noop_set_external_rendering),
                    active: Cell::new(false),
                    playing: Cell::new(false),
                    aspect_ratio: Cell::new(None),
                    command_channel: open_command_channel(host_id),
                })
            });
            host.active.set(active);
            host.playing.set(playing);
            host.aspect_ratio.set(aspect_ratio);

            if let Some(session) = &self.active_session
                && session.ivars().host.host_id == host_id
            {
                session.update_from_host_state();
            }
        }

        fn enter(&mut self, host_id: u64, mtm: MainThreadMarker) -> EnterResult {
            if !pip_available() {
                return EnterResult::Unsupported;
            }
            // SAFETY: `isPictureInPictureSupported` is a class query.
            if !unsafe { AVPictureInPictureController::isPictureInPictureSupported() } {
                return EnterResult::Unsupported;
            }
            let Some(host) = self.hosts.get(&host_id).cloned() else {
                return EnterResult::HostNotRegistered;
            };
            if host.user_data.is_null() {
                return EnterResult::HostNotRegistered;
            }

            if let Some(session) = &self.active_session
                && session.ivars().host.host_id == host_id
                && session.is_active()
            {
                return EnterResult::Success;
            }

            self.active_session = None;
            let Some(session) = PictureInPictureSession::new(host, mtm) else {
                return EnterResult::StartFailed;
            };
            let result = session.start();
            self.active_session = Some(session);
            result
        }

        fn session_did_stop(&mut self, host_id: u64) {
            if let Some(session) = &self.active_session
                && session.ivars().host.host_id == host_id
            {
                self.active_session = None;
            }
        }
    }

    impl std::fmt::Debug for PictureInPictureSessionIvars {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter
                .debug_struct("PictureInPictureSessionIvars")
                .field("host_id", &self.host.host_id)
                .finish_non_exhaustive()
        }
    }

    pub(super) fn register_gpu_surface_host(
        host_id: PictureInPictureHostId,
        user_data: *mut c_void,
        render_frame: ApplePictureInPictureRenderFrame,
        set_external_rendering: ApplePictureInPictureSetExternalRendering,
        mtm: MainThreadMarker,
    ) {
        if !pip_available() {
            return;
        }
        manager(mtm).borrow_mut().register_host(
            host_id.get(),
            user_data,
            render_frame,
            set_external_rendering,
        );
    }

    pub(super) fn unregister_gpu_surface_host(
        host_id: PictureInPictureHostId,
        mtm: MainThreadMarker,
    ) {
        if !pip_available() {
            return;
        }
        manager(mtm).borrow_mut().unregister_host(host_id.get());
    }

    pub(super) fn sync_picture_in_picture_controller(
        state: PictureInPictureControllerState,
        mtm: MainThreadMarker,
    ) {
        if !pip_available() {
            return;
        }
        manager(mtm).borrow_mut().sync_host_state(
            state.host_id.get(),
            state.active,
            state.playing,
            state.aspect_ratio,
        );
    }

    pub(super) async fn enter_picture_in_picture(
        host_id: PictureInPictureHostId,
        _aspect_ratio: Option<(u32, u32)>,
    ) -> Result<(), VideoError> {
        let result = waterkit_core::apple::on_main(move |mtm| {
            if !pip_available() {
                return EnterResult::Unsupported;
            }
            manager(mtm).borrow_mut().enter(host_id.get(), mtm)
        })
        .await;
        match result {
            EnterResult::Success => Ok(()),
            EnterResult::Unsupported => Err(VideoError::Unsupported(
                "Apple picture in picture is unavailable on this device".into(),
            )),
            EnterResult::HostNotRegistered => Err(VideoError::Unsupported(
                "Apple picture in picture host has not been registered".into(),
            )),
            EnterResult::NotPossible => Err(VideoError::Unsupported(
                "Apple picture in picture is currently not possible for this host".into(),
            )),
            EnterResult::StartFailed => Err(VideoError::Unsupported(
                "Apple picture in picture controller rejected the start request".into(),
            )),
        }
    }

    pub(super) fn open_picture_in_picture_command_channel(host_id: PictureInPictureHostId) {
        let _ = open_command_channel(host_id.get());
    }

    pub(super) fn close_picture_in_picture_command_channel(host_id: PictureInPictureHostId) {
        close_command_channel(host_id.get());
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "the caller destructures the channel wait as a fallible op"
    )]
    pub(super) fn wait_picture_in_picture_command(
        host_id: PictureInPictureHostId,
    ) -> Result<Option<PictureInPictureCommand>, VideoError> {
        let channel = {
            let channels = COMMAND_CHANNELS.lock().unwrap();
            channels.get(&host_id.get()).cloned()
        };
        let Some(channel) = channel else {
            return Ok(None);
        };
        Ok(channel.wait())
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
