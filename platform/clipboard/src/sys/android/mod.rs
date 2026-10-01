//! Android clipboard implementation using JNI and `ndk_context`.

use crate::content::{ClipboardEvent, Image};
use crate::error::ClipboardError;
use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{Global, JByteArray, JObject, JValue};
use jni::{Env, EnvUnowned, JavaVM, jni_sig, jni_str};
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use waterkit_build::{AndroidError, DexHelper, decode_string, dex_helper, jvm_and_context};

/// `waterkit.clipboard.ClipboardHelper`, compiled into the app's DEX by the
/// packager and resolved through the application's `ClassLoader`.
static HELPER: DexHelper = dex_helper!("waterkit.clipboard.ClipboardHelper");

/// `waterkit.clipboard.ClipboardWatchCallback`, the per-watcher
/// `OnPrimaryClipChangedListener` bridge, resolved the same way.
static WATCH_CALLBACK: DexHelper = dex_helper!("waterkit.clipboard.ClipboardWatchCallback");

impl From<AndroidError> for ClipboardError {
    fn from(error: AndroidError) -> Self {
        Self::Platform(error.to_string())
    }
}

/// Reads one `(Landroid/content/Context;)Z` probe on the helper.
fn probe(env: &mut Env<'_>, context: &JObject<'_>, name: &jni::strings::JNIStr) -> bool {
    let Ok(helper_class) = HELPER.class(env, context) else {
        return false;
    };
    env.call_static_method(
        helper_class,
        name,
        jni_sig!("(Landroid/content/Context;)Z"),
        &[JValue::Object(context)],
    )
    .and_then(jni::objects::JValueOwned::z)
    .unwrap_or(false)
}

fn read_byte_array(env: &Env<'_>, value: JObject<'_>) -> Result<Vec<u8>, ClipboardError> {
    let array = env
        .cast_local::<JByteArray>(value)
        .map_err(|e| ClipboardError::Platform(format!("JNI error byte array cast: {e}")))?;
    env.convert_byte_array(&array)
        .map_err(|e| ClipboardError::Platform(format!("JNI error convert_byte_array: {e}")))
}

/// Android clipboard handle.
#[derive(Debug)]
pub struct ClipboardInner {
    vm: JavaVM,
    context: Global<JObject<'static>>,
}

impl ClipboardInner {
    /// Create a new clipboard handle.
    ///
    /// # Panics
    ///
    /// Panics if Android context is not available via `ndk_context`.
    pub fn new() -> Result<Self, ClipboardError> {
        let (vm, context) = jvm_and_context()?;

        // Resolve the helper class up front so later calls are plain lookups.
        vm.attach_current_thread(
            |env| -> Result<Result<(), ClipboardError>, jni::errors::Error> {
                Ok(HELPER
                    .class(env, context.as_obj())
                    .map(|_| ())
                    .map_err(ClipboardError::from))
            },
        )
        .map_err(|e| ClipboardError::Platform(format!("JNI attach error: {e}")))??;

        Ok(Self { vm, context })
    }

    fn with_env<T, F>(&self, f: F) -> Result<T, ClipboardError>
    where
        F: FnOnce(&mut Env<'_>, &JObject<'_>) -> Result<T, ClipboardError>,
    {
        self.vm
            .attach_current_thread(
                |env| -> Result<Result<T, ClipboardError>, jni::errors::Error> {
                    Ok(f(env, self.context.as_obj()))
                },
            )
            .map_err(|e| ClipboardError::Platform(format!("JNI attach error: {e}")))?
    }

    /// Calls a helper method that returns a nullable `java.lang.String`.
    fn read_optional_string(
        &self,
        method: &'static jni::strings::JNIStr,
        what: &'static str,
    ) -> Result<Option<String>, ClipboardError> {
        self.with_env(|env, context| {
            let helper_class = HELPER.class(env, context)?;

            let value = env
                .call_static_method(
                    helper_class,
                    method,
                    jni_sig!("(Landroid/content/Context;)Ljava/lang/String;"),
                    &[JValue::Object(context)],
                )
                .map_err(|e| ClipboardError::Platform(format!("JNI error {what}: {e}")))?
                .l()
                .map_err(|e| ClipboardError::Platform(format!("JNI error result: {e}")))?;

            if value.is_null() {
                Ok(None)
            } else {
                decode_string(env, &value)
                    .map(Some)
                    .map_err(ClipboardError::from)
            }
        })
    }

    // ========== Query (sync) ==========

    /// Check if text is available.
    pub fn has_text(&self) -> bool {
        self.with_env(|env, context| Ok(probe(env, context, jni_str!("hasText"))))
            .unwrap_or(false)
    }

    /// Check if HTML is available.
    pub fn has_html(&self) -> bool {
        self.with_env(|env, context| Ok(probe(env, context, jni_str!("hasHtml"))))
            .unwrap_or(false)
    }

    /// Check if files are available.
    pub fn has_files(&self) -> bool {
        self.with_env(|env, context| Ok(probe(env, context, jni_str!("hasFiles"))))
            .unwrap_or(false)
    }

    /// Check if image is available.
    pub fn has_image(&self) -> bool {
        self.with_env(|env, context| Ok(probe(env, context, jni_str!("hasImage"))))
            .unwrap_or(false)
    }

    // ========== Read (sync, called from blocking::unblock) ==========

    /// Get text content.
    pub fn get_text(&self) -> Result<Option<String>, ClipboardError> {
        self.read_optional_string(jni_str!("getText"), "getText")
    }

    /// Get HTML content.
    pub fn get_html(&self) -> Result<Option<String>, ClipboardError> {
        self.read_optional_string(jni_str!("getHtml"), "getHtml")
    }

    /// Get file paths.
    pub fn get_files(&self) -> Result<Vec<PathBuf>, ClipboardError> {
        let Some(url) = self.read_optional_string(jni_str!("getFileUri"), "getFileUri")? else {
            return Ok(Vec::new());
        };

        if let Some(path) = url.strip_prefix("file://") {
            let decoded = percent_encoding::percent_decode_str(path)
                .decode_utf8()
                .map_err(|e| ClipboardError::Platform(format!("Invalid URL encoding: {e}")))?;
            Ok(vec![PathBuf::from(decoded.into_owned())])
        } else {
            Ok(Vec::new())
        }
    }

    /// Get image as RGBA.
    pub fn get_image(&self) -> Result<Option<Image>, ClipboardError> {
        self.with_env(|env, context| {
            let helper_class = HELPER.class(env, context)?;

            let width = env
                .call_static_method(
                    helper_class,
                    jni_str!("getImageWidth"),
                    jni_sig!("(Landroid/content/Context;)I"),
                    &[JValue::Object(context)],
                )
                .and_then(jni::objects::JValueOwned::i)
                .unwrap_or(-1);

            if width <= 0 {
                return Ok(None);
            }

            let height = env
                .call_static_method(
                    helper_class,
                    jni_str!("getImageHeight"),
                    jni_sig!("(Landroid/content/Context;)I"),
                    &[JValue::Object(context)],
                )
                .and_then(jni::objects::JValueOwned::i)
                .unwrap_or(-1);

            if height <= 0 {
                return Ok(None);
            }

            let bytes = env
                .call_static_method(
                    helper_class,
                    jni_str!("getImageRgba"),
                    jni_sig!("(Landroid/content/Context;)[B"),
                    &[JValue::Object(context)],
                )
                .map_err(|e| ClipboardError::Platform(format!("JNI error getImageRgba: {e}")))?
                .l()
                .map_err(|e| ClipboardError::Platform(format!("JNI error result: {e}")))?;

            if bytes.is_null() {
                return Ok(None);
            }

            let bytes = read_byte_array(env, bytes)?;

            Ok(Some(Image::new(
                width.cast_unsigned(),
                height.cast_unsigned(),
                bytes,
            )))
        })
    }

    /// Get binary data by MIME type.
    pub fn get_binary(&self, mime: &str) -> Result<Option<Vec<u8>>, ClipboardError> {
        let mime = mime.to_string();
        self.with_env(|env, context| {
            let helper_class = HELPER.class(env, context)?;

            let jmime = env
                .new_string(&mime)
                .map_err(|e| ClipboardError::Platform(format!("JNI error new_string: {e}")))?;

            let value = env
                .call_static_method(
                    helper_class,
                    jni_str!("getBinary"),
                    jni_sig!("(Landroid/content/Context;Ljava/lang/String;)[B"),
                    &[JValue::Object(context), JValue::Object(&jmime)],
                )
                .map_err(|e| ClipboardError::Platform(format!("JNI error getBinary: {e}")))?
                .l()
                .map_err(|e| ClipboardError::Platform(format!("JNI error result: {e}")))?;

            if value.is_null() {
                return Ok(None);
            }

            read_byte_array(env, value).map(Some)
        })
    }

    // ========== Write (sync) ==========

    /// Set text content.
    pub fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        let text = text.to_string();
        self.with_env(|env, context| {
            let helper_class = HELPER.class(env, context)?;

            let jtext = env
                .new_string(&text)
                .map_err(|e| ClipboardError::Platform(format!("JNI error new_string: {e}")))?;

            env.call_static_method(
                helper_class,
                jni_str!("setText"),
                jni_sig!("(Landroid/content/Context;Ljava/lang/String;)V"),
                &[JValue::Object(context), JValue::Object(&jtext)],
            )
            .map_err(|e| ClipboardError::Platform(format!("JNI error setText: {e}")))?;

            Ok(())
        })
    }

    /// Set HTML content.
    pub fn set_html(&self, html: &str, alt_text: Option<&str>) -> Result<(), ClipboardError> {
        let html = html.to_string();
        let alt = alt_text.unwrap_or("").to_string();
        self.with_env(|env, context| {
            let helper_class = HELPER.class(env, context)?;

            let jhtml = env
                .new_string(&html)
                .map_err(|e| ClipboardError::Platform(format!("JNI error new_string html: {e}")))?;
            let jalt = env
                .new_string(&alt)
                .map_err(|e| ClipboardError::Platform(format!("JNI error new_string alt: {e}")))?;

            env.call_static_method(
                helper_class,
                jni_str!("setHtml"),
                jni_sig!("(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)V"),
                &[
                    JValue::Object(context),
                    JValue::Object(&jhtml),
                    JValue::Object(&jalt),
                ],
            )
            .map_err(|e| ClipboardError::Platform(format!("JNI error setHtml: {e}")))?;

            Ok(())
        })
    }

    /// Set file paths.
    pub fn set_files(&self, files: &[PathBuf]) -> Result<(), ClipboardError> {
        if files.is_empty() {
            return Ok(());
        }
        // Android only supports single file URI
        let path = &files[0];
        let url = format!(
            "file://{}",
            percent_encoding::utf8_percent_encode(
                path.to_string_lossy().as_ref(),
                percent_encoding::NON_ALPHANUMERIC
            )
        );

        self.with_env(|env, context| {
            let helper_class = HELPER.class(env, context)?;

            let juri = env
                .new_string(&url)
                .map_err(|e| ClipboardError::Platform(format!("JNI error new_string: {e}")))?;

            env.call_static_method(
                helper_class,
                jni_str!("setFileUri"),
                jni_sig!("(Landroid/content/Context;Ljava/lang/String;)V"),
                &[JValue::Object(context), JValue::Object(&juri)],
            )
            .map_err(|e| ClipboardError::Platform(format!("JNI error setFileUri: {e}")))?;

            Ok(())
        })
    }

    /// Set image from a file path.
    pub fn set_image_from_path(&self, path: &Path) -> Result<(), ClipboardError> {
        let path_str = path.to_string_lossy().to_string();
        self.with_env(|env, context| {
            let helper_class = HELPER.class(env, context)?;

            let jpath = env
                .new_string(&path_str)
                .map_err(|e| ClipboardError::Platform(format!("JNI error new_string: {e}")))?;

            let success = env
                .call_static_method(
                    helper_class,
                    jni_str!("setImageFromPath"),
                    jni_sig!("(Landroid/content/Context;Ljava/lang/String;)Z"),
                    &[JValue::Object(context), JValue::Object(&jpath)],
                )
                .and_then(jni::objects::JValueOwned::z)
                .unwrap_or(false);

            if !success {
                return Err(ClipboardError::InvalidImage(
                    "failed to load image from path".into(),
                ));
            }

            Ok(())
        })
    }

    /// Set binary data with MIME type.
    pub fn set_binary(&self, data: &[u8], mime: &str) -> Result<(), ClipboardError> {
        let data = data.to_vec();
        let mime = mime.to_string();
        self.with_env(|env, context| {
            let helper_class = HELPER.class(env, context)?;

            let jdata = env
                .byte_array_from_slice(&data)
                .map_err(|e| ClipboardError::Platform(format!("JNI error byte_array: {e}")))?;
            let jmime = env
                .new_string(&mime)
                .map_err(|e| ClipboardError::Platform(format!("JNI error new_string: {e}")))?;

            env.call_static_method(
                helper_class,
                jni_str!("setBinary"),
                jni_sig!("(Landroid/content/Context;[BLjava/lang/String;)V"),
                &[
                    JValue::Object(context),
                    JValue::Object(jdata.as_ref()),
                    JValue::Object(&jmime),
                ],
            )
            .map_err(|e| ClipboardError::Platform(format!("JNI error setBinary: {e}")))?;

            Ok(())
        })
    }

    /// Set file promise.
    ///
    /// On Android, file promises are not supported, so this falls back
    /// to immediately calling the provider and setting the file URI.
    pub fn set_file_promise(
        &self,
        provider: Box<dyn FnOnce() -> Result<PathBuf, ClipboardError> + Send>,
    ) -> Result<(), ClipboardError> {
        let path = provider()?;
        self.set_files(&[path])
    }

    /// Clear clipboard.
    pub fn clear(&self) -> Result<(), ClipboardError> {
        self.with_env(|env, context| {
            let helper_class = HELPER.class(env, context)?;

            env.call_static_method(
                helper_class,
                jni_str!("clear"),
                jni_sig!("(Landroid/content/Context;)V"),
                &[JValue::Object(context)],
            )
            .map_err(|e| ClipboardError::Platform(format!("JNI error clear: {e}")))?;

            Ok(())
        })
    }
}

/// Rust-side state shared with one registered `ClipboardWatchCallback`.
///
/// The Kotlin object holds this address in its `waterkit_watch_state` field
/// and dereferences it inside `onPrimaryClipChanged`, which is serialized
/// with `releaseNativeState` on the object's monitor: once release returns,
/// no queued clip notification can take another reference.
#[derive(Debug)]
struct WatchCallbackState {
    sender: async_channel::Sender<ClipboardEvent>,
}

// JNI longs transport pointer bits, including Android's high-bit memory tags.
fn watch_state_handle(state: &Arc<WatchCallbackState>) -> i64 {
    Arc::as_ptr(state) as i64
}

/// Reborrows the callback's shared state from its `waterkit_watch_state`
/// field. Callable only inside `onPrimaryClipChanged`, which holds the
/// callback object's monitor.
fn watch_state(env: &mut Env<'_>, callback: &JObject) -> Arc<WatchCallbackState> {
    let value = env
        .get_field(callback, jni_str!("waterkit_watch_state"), jni_sig!("J"))
        .unwrap_or_else(|error| {
            panic!("waterkit-clipboard: read waterkit_watch_state failed in clip callback: {error}")
        });
    let state_handle = value.j().unwrap_or_else(|error| {
        panic!("waterkit-clipboard: decode waterkit_watch_state failed in clip callback: {error}")
    });
    // The private callback receives exactly the pointer bits written at
    // registration. On 32-bit Android, narrowing restores the original
    // pointer width; on 64-bit Android, it preserves the memory tag too.
    let state = state_handle as *const WatchCallbackState;
    // SAFETY: `onPrimaryClipChanged` runs under the callback object's
    // monitor, serialized with `releaseNativeState`; the owning
    // `WatchSession` keeps the original Arc alive until that synchronized
    // release returns.
    unsafe {
        Arc::increment_strong_count(state);
        Arc::from_raw(state)
    }
}

/// Owns one registered `OnPrimaryClipChangedListener` and the Rust state it
/// reports to.
///
/// Dropping the session unregisters the listener and releases the callback
/// state, so the stream's channel closes once `stop()` runs or the owning
/// `WatcherShutdown` is dropped.
#[derive(Debug)]
pub struct WatchSession {
    vm: JavaVM,
    context: Global<JObject<'static>>,
    registration: Option<WatchRegistration>,
}

/// Owns the allocation while Java may dereference it. `state` becomes `None`
/// immediately after synchronized native release, independently of listener
/// removal or attachment-scope completion. Until then, unwinding must retain
/// the allocation, even if the caller catches the cleanup panic.
#[derive(Debug)]
struct WatchRegistration {
    callback: Global<JObject<'static>>,
    state: Option<ManuallyDrop<Arc<WatchCallbackState>>>,
}

/// Preserve a Java throwable's diagnostics and clear it before further JNI
/// work. Querying the throwable can itself throw, so clear that exception too.
fn watch_jni_error(env: &Env<'_>, operation: &str, error: &jni::errors::Error) -> ClipboardError {
    let caught = env.exception_catch().err();
    env.exception_clear();
    let detail = caught.map_or_else(
        || error.to_string(),
        |caught| format!("{error}; {caught:?}"),
    );
    ClipboardError::Platform(format!("{operation}: {detail}"))
}

/// Tear down a watch's Java side while `state` is still owned.
///
/// Order is load-bearing: `releaseNativeState` zeroes `waterkit_watch_state`
/// under the same callback monitor that serializes
/// `onPrimaryClipChanged`, so once it returns no queued clip notification
/// can take another reference into `state`. Only then may `state` be
/// dropped. Unregistering the listener comes last.
///
/// If release fails, the registration retains its allocation through unwind;
/// callers report cleanup failures by panic. If removal fails after release,
/// the listener is inert and the allocation has already been dropped.
fn teardown_watch(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    registration: &mut WatchRegistration,
) -> Result<(), ClipboardError> {
    env.call_method(
        registration.callback.as_obj(),
        jni_str!("releaseNativeState"),
        jni_sig!("()V"),
        &[],
    )
    .map_err(|error| watch_jni_error(env, "releaseNativeState", &error))?;
    drop(registration.state.take().map(ManuallyDrop::into_inner));

    let helper = HELPER.class(env, context).map_err(|error| {
        ClipboardError::Platform(format!(
            "resolve clipboard helper for listener removal: {error}"
        ))
    })?;
    env.call_static_method(
        helper,
        jni_str!("stopWatching"),
        jni_sig!(
            "(Landroid/content/Context;Landroid/content/ClipboardManager$OnPrimaryClipChangedListener;)V"
        ),
        &[
            JValue::Object(context),
            JValue::Object(registration.callback.as_obj()),
        ],
    )
    .map_err(|error| watch_jni_error(env, "stopWatching", &error))?;
    Ok(())
}

impl WatchSession {
    /// Stop watching: release the callback's native state under its monitor,
    /// unregister the listener, and drop our JNI references.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        let Some(mut registration) = self.registration.take() else {
            return;
        };

        // Keep the operation result outside the closure: attachment-scope
        // completion may fail after teardown has already run.
        let mut teardown = None;
        let attachment = self
            .vm
            .attach_current_thread(|env| -> jni::errors::Result<()> {
                teardown = Some(teardown_watch(
                    env,
                    self.context.as_obj(),
                    &mut registration,
                ));
                Ok(())
            });
        if let Err(error) = attachment {
            let phase = if registration.state.is_some() {
                "before native state release; allocation retained"
            } else {
                "after native state release"
            };
            panic!(
                "waterkit-clipboard: JVM attachment scope for watch teardown failed {phase}: {error}; teardown result: {teardown:?}"
            );
        }
        if let Some(Err(error)) = teardown {
            panic!("waterkit-clipboard: watch teardown failed: {error}");
        }
    }
}

impl Drop for WatchSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn register_watch(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    state: Arc<WatchCallbackState>,
) -> Result<WatchRegistration, ClipboardError> {
    let helper = HELPER.class(env, context)?;
    let callback_class = WATCH_CALLBACK.class(env, context)?;

    let callback = env
        .new_object(
            callback_class,
            jni_sig!("(Landroid/content/Context;)V"),
            &[JValue::Object(context)],
        )
        .map_err(|error| watch_jni_error(env, "new ClipboardWatchCallback", &error))?;

    env.set_field(
        &callback,
        jni_str!("waterkit_watch_state"),
        jni_sig!("J"),
        JValue::Long(watch_state_handle(&state)),
    )
    .map_err(|error| watch_jni_error(env, "set waterkit_watch_state", &error))?;

    // Globalize before registering: once `startWatching` succeeds the
    // listener is armed and every later failure must still reach this
    // callback for teardown.
    let callback = env
        .new_global_ref(&callback)
        .map_err(|error| watch_jni_error(env, "new_global_ref callback", &error))?;

    // From this point Java may publish the pointer even if registration
    // throws. Only confirmed release (or false before registration) permits
    // dropping this Arc, including on Rust unwind.
    let mut registration = WatchRegistration {
        callback,
        state: Some(ManuallyDrop::new(state)),
    };

    match env
        .call_static_method(
            helper,
            jni_str!("startWatching"),
            jni_sig!(
                "(Landroid/content/Context;Landroid/content/ClipboardManager$OnPrimaryClipChangedListener;)Z"
            ),
            &[
                JValue::Object(context),
                JValue::Object(registration.callback.as_obj()),
            ],
        )
        .and_then(jni::objects::JValueOwned::z)
    {
        Ok(true) => Ok(registration),
        // `startWatching` returns false only before adding the listener, so
        // the armed pointer is unreachable and `state` drops normally.
        Ok(false) => {
            drop(registration.state.take().map(ManuallyDrop::into_inner));
            Err(ClipboardError::Unavailable)
        }
        Err(error) => {
            // The call may have registered the listener before throwing:
            // capture and clear the throwable before making any cleanup call.
            let error = watch_jni_error(env, "startWatching", &error);
            if let Err(cleanup) = teardown_watch(env, context, &mut registration) {
                panic!(
                    "waterkit-clipboard: registration failed: {error}; watch teardown also failed: {cleanup}"
                );
            }
            Err(error)
        }
    }
}

/// Start watching clipboard changes.
///
/// Registers a `ClipboardManager.OnPrimaryClipChangedListener` (available on
/// every API level `WaterKit` supports; callbacks arrive on the main thread).
/// Every clip notification emits a [`ClipboardEvent`], including changes
/// whose type set matches the previous clip.
///
/// Returns a receiver that yields `ClipboardEvent`s and the session that
/// owns the registered listener.
///
/// # Errors
///
/// Returns [`ClipboardError::Unavailable`] if the clipboard service cannot be
/// reached, or [`ClipboardError::Platform`] if the JNI bridge fails.
///
/// # Panics
///
/// Panics if the Android context is not available via `ndk_context`.
pub fn start_watch()
-> Result<(async_channel::Receiver<ClipboardEvent>, WatchSession), ClipboardError> {
    let (vm, context) = jvm_and_context().unwrap_or_else(|error| {
        panic!("waterkit-clipboard: failed to resolve the Android context for watching: {error}")
    });

    let (sender, receiver) = async_channel::unbounded();
    let state = Arc::new(WatchCallbackState { sender });

    let mut session = WatchSession {
        vm,
        context,
        registration: None,
    };
    let mut registration_error = None;
    let attachment = session
        .vm
        .attach_current_thread(|env| -> jni::errors::Result<()> {
            match register_watch(env, session.context.as_obj(), state) {
                Ok(registration) => session.registration = Some(registration),
                Err(error) => registration_error = Some(error),
            }
            Ok(())
        });
    // The session owns a successful registration before attachment-scope
    // completion. If that completion fails or unwinds, Drop still tears it
    // down instead of dropping an Arc whose address Java can still reach.
    if let Err(error) = attachment {
        return Err(ClipboardError::Platform(format!(
            "JVM attachment scope for watch registration failed: {error}; registration error: {registration_error:?}"
        )));
    }
    if let Some(error) = registration_error {
        return Err(error);
    }
    Ok((receiver, session))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_clipboard_ClipboardWatchCallback_onPrimaryClipChangedNative<
    'caller,
>(
    mut env: EnvUnowned<'caller>,
    callback: JObject<'caller>,
    has_text: jni::sys::jboolean,
    has_html: jni::sys::jboolean,
    has_files: jni::sys::jboolean,
    has_image: jni::sys::jboolean,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        let state = watch_state(env, &callback);
        // A failed send means the receiver is gone; the session's stop path
        // releases this state shortly after, so dropping the event is the
        // extent of the failure.
        let _ = state.sender.try_send(ClipboardEvent::new(
            has_text, has_html, has_files, has_image,
        ));
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}
