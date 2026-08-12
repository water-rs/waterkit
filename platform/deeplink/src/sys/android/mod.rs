use crate::{DeepLink, DeepLinkError};
use jni::objects::{Global, JObject};
use jni::{Env, JavaVM};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Returns the application's JVM together with a global reference to its
/// Android `Context`, both published by `ndk_context`.
fn context_global() -> Result<(JavaVM, Global<JObject<'static>>), DeepLinkError> {
    let android_context = ndk_context::android_context();
    let raw_vm: *mut jni::sys::JavaVM = android_context.vm().cast();
    let raw_context: jni::sys::jobject = android_context.context().cast();
    assert!(
        !raw_vm.is_null(),
        "waterkit-deeplink: ndk_context returned a null JavaVM"
    );
    assert!(
        !raw_context.is_null(),
        "waterkit-deeplink: ndk_context returned a null Android Context"
    );

    // SAFETY: `ndk_context` publishes the process' JavaVM pointer, which stays
    // valid for the lifetime of the application.
    let vm = unsafe { JavaVM::from_raw(raw_vm) };
    let context = vm
        .attach_current_thread(|env| -> jni::errors::Result<Global<JObject<'static>>> {
            // SAFETY: `ndk_context` publishes a global reference to the
            // application `Context` that outlives this attachment, and
            // `as_cast_raw` only borrows it.
            let context = unsafe { env.as_cast_raw::<JObject>(&raw_context)? };
            env.new_global_ref(&*context)
        })
        .map_err(|e| DeepLinkError::Platform(format!("new_global_ref context: {e}")))?;

    Ok((vm, context))
}

/// Runs `f` with the calling thread attached to the application's JVM and the
/// Android `Context`.
fn with_android_context<T, F>(f: F) -> Result<T, DeepLinkError>
where
    F: FnOnce(&mut Env<'_>, &JObject<'_>) -> Result<T, DeepLinkError>,
{
    let (vm, context) = context_global()?;
    vm.attach_current_thread(
        |env| -> Result<Result<T, DeepLinkError>, jni::errors::Error> {
            Ok(f(env, context.as_obj()))
        },
    )
    .map_err(|e| DeepLinkError::Platform(format!("attach_current_thread: {e}")))?
}

pub mod jni_api {
    use super::{DeepLinkError, Env, JObject};
    use jni::objects::{JString, JValue};
    use jni::{jni_sig, jni_str};

    /// Builds an `Intent.ACTION_VIEW` for `url`.
    fn view_intent<'local>(
        env: &mut Env<'local>,
        url: &str,
    ) -> Result<JObject<'local>, DeepLinkError> {
        let action_view = env
            .new_string("android.intent.action.VIEW")
            .map_err(|e| DeepLinkError::Platform(format!("new action string: {e}")))?;
        let url_j = env
            .new_string(url)
            .map_err(|e| DeepLinkError::Platform(format!("new url string: {e}")))?;

        let uri = env
            .call_static_method(
                jni_str!("android/net/Uri"),
                jni_str!("parse"),
                jni_sig!("(Ljava/lang/String;)Landroid/net/Uri;"),
                &[JValue::Object(&url_j)],
            )
            .and_then(jni::objects::JValueOwned::l)
            .map_err(|e| DeepLinkError::Platform(format!("Uri.parse: {e}")))?;

        env.new_object(
            jni_str!("android/content/Intent"),
            jni_sig!("(Ljava/lang/String;Landroid/net/Uri;)V"),
            &[JValue::Object(&action_view), JValue::Object(&uri)],
        )
        .map_err(|e| DeepLinkError::Platform(format!("new Intent: {e}")))
    }

    /// Check whether a URL can be opened with the given Android context.
    ///
    /// # Errors
    ///
    /// Returns error when JNI calls fail.
    pub fn can_open_url_with_context(
        env: &mut Env<'_>,
        context: &JObject<'_>,
        url: &str,
    ) -> Result<bool, DeepLinkError> {
        let intent = view_intent(env, url)?;

        let package_manager = env
            .call_method(
                context,
                jni_str!("getPackageManager"),
                jni_sig!("()Landroid/content/pm/PackageManager;"),
                &[],
            )
            .and_then(jni::objects::JValueOwned::l)
            .map_err(|e| DeepLinkError::Platform(format!("getPackageManager: {e}")))?;

        let resolved = env
            .call_method(
                &package_manager,
                jni_str!("resolveActivity"),
                jni_sig!("(Landroid/content/Intent;I)Landroid/content/pm/ResolveInfo;"),
                &[JValue::Object(&intent), JValue::Int(0)],
            )
            .and_then(jni::objects::JValueOwned::l)
            .map_err(|e| DeepLinkError::Platform(format!("resolveActivity: {e}")))?;

        Ok(!resolved.is_null())
    }

    /// Open a URL with the given Android context.
    ///
    /// # Errors
    ///
    /// Returns error when JNI calls fail.
    pub fn open_url_with_context(
        env: &mut Env<'_>,
        context: &JObject<'_>,
        url: &str,
    ) -> Result<(), DeepLinkError> {
        let intent = view_intent(env, url)?;

        // Intent.FLAG_ACTIVITY_NEW_TASK
        let flag = 0x1000_0000_i32;
        env.call_method(
            &intent,
            jni_str!("addFlags"),
            jni_sig!("(I)Landroid/content/Intent;"),
            &[JValue::Int(flag)],
        )
        .map_err(|e| DeepLinkError::Platform(format!("addFlags: {e}")))?;

        env.call_method(
            context,
            jni_str!("startActivity"),
            jni_sig!("(Landroid/content/Intent;)V"),
            &[JValue::Object(&intent)],
        )
        .map_err(|e| DeepLinkError::Platform(format!("startActivity: {e}")))?;

        Ok(())
    }

    /// Read deeplink URL from the current Activity intent.
    ///
    /// # Errors
    ///
    /// Returns error when JNI calls fail.
    pub fn intent_url_with_context(
        env: &mut Env<'_>,
        context: &JObject<'_>,
    ) -> Result<Option<String>, DeepLinkError> {
        let intent = env
            .call_method(
                context,
                jni_str!("getIntent"),
                jni_sig!("()Landroid/content/Intent;"),
                &[],
            )
            .and_then(jni::objects::JValueOwned::l)
            .map_err(|e| DeepLinkError::Platform(format!("getIntent: {e}")))?;
        if intent.is_null() {
            return Ok(None);
        }

        let data = env
            .call_method(
                &intent,
                jni_str!("getDataString"),
                jni_sig!("()Ljava/lang/String;"),
                &[],
            )
            .and_then(jni::objects::JValueOwned::l)
            .map_err(|e| DeepLinkError::Platform(format!("getDataString: {e}")))?;
        if data.is_null() {
            return Ok(None);
        }

        let data_string = env
            .as_cast::<JString>(&data)
            .and_then(|value| value.try_to_string(env))
            .map_err(|e| DeepLinkError::Platform(format!("decode dataString: {e}")))?;
        Ok(Some(data_string))
    }
}

#[allow(clippy::unused_async)]
pub async fn open_url(url: &str) -> Result<(), DeepLinkError> {
    with_android_context(|env, context| jni_api::open_url_with_context(env, context, url))
}

#[allow(clippy::unused_async)]
pub async fn can_open_url(url: &str) -> Result<bool, DeepLinkError> {
    with_android_context(|env, context| jni_api::can_open_url_with_context(env, context, url))
}

pub struct DeepLinkHandlerInner {
    stop_flag: Arc<AtomicBool>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for DeepLinkHandlerInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeepLinkHandlerInner").finish()
    }
}

impl DeepLinkHandlerInner {
    #[expect(
        clippy::unused_async_trait_impl,
        reason = "the cross-platform facade calls this entry point as async; other platforms await inside it"
    )]
    #[allow(clippy::unused_async)]
    pub async fn start() -> Result<(Self, async_channel::Receiver<DeepLink>), DeepLinkError> {
        let (vm, context) = context_global()?;
        let (link_tx, link_rx) = async_channel::unbounded();
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop_flag);

        let worker = std::thread::Builder::new()
            .name("waterkit-deeplink-android".to_owned())
            .spawn(move || {
                vm.attach_current_thread(|env| -> jni::errors::Result<()> {
                    let mut last_url: Option<String> = None;
                    while !stop_for_thread.load(Ordering::Relaxed) {
                        match jni_api::intent_url_with_context(env, context.as_obj()) {
                            Ok(Some(url)) if last_url.as_deref() != Some(url.as_str()) => {
                                if let Ok(link) = DeepLink::parse(&url)
                                    && link_tx.try_send(link).is_err()
                                {
                                    break;
                                }
                                last_url = Some(url);
                            }
                            Ok(_) => {}
                            Err(error) => {
                                panic!(
                                    "waterkit-deeplink: failed to read intent URL in listener thread: {error}"
                                );
                            }
                        }

                        std::thread::sleep(Duration::from_millis(250));
                    }
                    Ok(())
                })
                .unwrap_or_else(|error| {
                    panic!(
                        "waterkit-deeplink: attach_current_thread failed in listener thread: {error}"
                    )
                });
            })
            .map_err(|e| DeepLinkError::Platform(format!("spawn deeplink listener: {e}")))?;

        Ok((
            Self {
                stop_flag,
                worker: Mutex::new(Some(worker)),
            },
            link_rx,
        ))
    }

    pub fn initial_link(&self) -> Result<Option<DeepLink>, DeepLinkError> {
        let _ = &self.stop_flag;
        with_android_context(jni_api::intent_url_with_context)?
            .map(|url| DeepLink::parse(&url))
            .transpose()
    }

    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Ok(mut worker) = self.worker.lock()
            && let Some(handle) = worker.take()
        {
            let _ = handle.join();
        }
    }
}

impl Drop for DeepLinkHandlerInner {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Ok(worker) = self.worker.get_mut()
            && let Some(handle) = worker.take()
        {
            let _ = handle.join();
        }
    }
}
