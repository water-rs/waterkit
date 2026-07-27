//! Instance-scoped Android video output surfaces.

use std::sync::Arc;

use jni::{
    Env, JavaVM, jni_sig, jni_str,
    objects::{Global, JObject},
};
use waterkit_video_core::Error;

const ANDROID_MINIMUM_VIDEO_API: i32 = 24;

type GlobalObjectRef = Global<JObject<'static>>;

#[derive(Debug)]
struct AttachedVideoError(Error);

impl std::fmt::Display for AttachedVideoError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for AttachedVideoError {}

impl From<jni::errors::Error> for AttachedVideoError {
    fn from(error: jni::errors::Error) -> Self {
        Self(Error::Platform(error.to_string()))
    }
}

pub(crate) fn with_attached_env<T>(
    vm: &JavaVM,
    operation: impl FnOnce(&mut Env<'_>) -> Result<T, Error>,
) -> Result<T, Error> {
    vm.attach_current_thread(|env| operation(env).map_err(AttachedVideoError))
        .map_err(|error| error.0)
}

/// Android `Surface` retained together with the JVM that owns it.
///
/// This type deliberately carries no DRM or security claim. Callers that need
/// protected pixels must establish that invariant before wrapping it in the
/// platform-CDM surface type.
#[derive(Clone)]
pub struct AndroidVideoSurface {
    pub(crate) context: AndroidPlaybackContext,
    pub(crate) surface: Arc<GlobalObjectRef>,
}

/// Instance-scoped Android media context.
#[derive(Clone)]
pub struct AndroidPlaybackContext {
    pub(crate) vm: Arc<JavaVM>,
}

impl AndroidPlaybackContext {
    /// Retains the JVM used by Android media objects.
    ///
    /// # Safety
    ///
    /// `env` must belong to the application JVM that owns subsequent media objects.
    ///
    /// # Errors
    ///
    /// Returns an error when Android is below API 24 or the JVM cannot be retained.
    pub unsafe fn from_jni(env: &mut Env<'_>) -> Result<Self, Error> {
        let api_level = android_api_level(env)?;
        if api_level < ANDROID_MINIMUM_VIDEO_API {
            return Err(Error::Unsupported(format!(
                "Android media playback requires API {ANDROID_MINIMUM_VIDEO_API} or newer, got {api_level}"
            )));
        }
        let vm = env
            .get_java_vm()
            .map_err(|error| jni_error(env, "retain Android media JavaVM", error))?;
        Ok(Self { vm: Arc::new(vm) })
    }
}

impl std::fmt::Debug for AndroidPlaybackContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AndroidPlaybackContext")
            .finish_non_exhaustive()
    }
}

impl AndroidVideoSurface {
    /// Retains an Android `Surface` for a decoder owned by the same JVM.
    ///
    /// # Safety
    ///
    /// `surface` must remain a valid `android.view.Surface` for the playback
    /// host lifetime and must belong to the JVM represented by `env`.
    ///
    /// # Errors
    ///
    /// Returns an error when Android is below API 24, the object is not a
    /// `Surface`, or the JVM/global reference cannot be retained.
    pub unsafe fn from_jni(env: &mut Env<'_>, surface: &JObject<'_>) -> Result<Self, Error> {
        let context = unsafe { AndroidPlaybackContext::from_jni(env) }?;
        if !env
            .is_instance_of(surface, jni_str!("android/view/Surface"))
            .map_err(|error| jni_error(env, "validate Android video Surface", error))?
        {
            return Err(Error::Platform(String::from(
                "Android video output object is not android.view.Surface",
            )));
        }
        let surface = env
            .new_global_ref(surface)
            .map_err(|error| jni_error(env, "retain Android video Surface", error))?;
        Ok(Self {
            context,
            surface: Arc::new(surface),
        })
    }

    /// Returns the media context that owns this surface.
    #[must_use]
    pub const fn context(&self) -> &AndroidPlaybackContext {
        &self.context
    }
}

impl std::fmt::Debug for AndroidVideoSurface {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AndroidVideoSurface")
            .finish_non_exhaustive()
    }
}

pub fn android_api_level(env: &mut Env<'_>) -> Result<i32, Error> {
    env.get_static_field(
        jni_str!("android/os/Build$VERSION"),
        jni_str!("SDK_INT"),
        jni_sig!("I"),
    )
    .and_then(jni::objects::JValueOwned::i)
    .map_err(|error| jni_error(env, "read Android API level", error))
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "this adapter is passed directly to JNI map_err closures"
)]
pub fn jni_error(env: &mut Env<'_>, operation: &str, error: jni::errors::Error) -> Error {
    let exception = env.exception_occurred();
    if exception.is_some() {
        env.exception_clear();
    }
    let detail = exception
        .and_then(|exception| {
            env.call_method(
                &exception,
                jni_str!("toString"),
                jni_sig!("()Ljava/lang/String;"),
                &[],
            )
            .ok()
            .and_then(|value| value.l().ok())
            .and_then(|value| (!value.is_null()).then_some(value))
            .and_then(|value| {
                env.as_cast::<jni::objects::JString>(&value)
                    .ok()
                    .and_then(|value| value.try_to_string(env).ok())
            })
        })
        .unwrap_or_else(|| error.to_string());
    Error::Platform(format!("{operation}: {detail}"))
}
