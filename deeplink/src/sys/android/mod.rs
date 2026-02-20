use crate::{DeepLink, DeepLinkError};
use jni::objects::{GlobalRef, JObject, JString, JValue};
use jni::{JNIEnv, JavaVM};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

fn get_vm_and_context() -> Result<(JavaVM, JObject<'static>), DeepLinkError> {
    let android_ctx = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(android_ctx.vm().cast()) }
        .map_err(|e| DeepLinkError::PlatformError(format!("from_raw vm: {e}")))?;
    let context = unsafe { JObject::from_raw(android_ctx.context().cast()) };
    Ok((vm, context))
}

fn ensure_context_global() -> Result<(JavaVM, GlobalRef), DeepLinkError> {
    let (vm, context) = get_vm_and_context()?;
    let global = {
        let env = vm
            .attach_current_thread()
            .map_err(|e| DeepLinkError::PlatformError(format!("attach_current_thread: {e}")))?;
        env.new_global_ref(&context)
            .map_err(|e| DeepLinkError::PlatformError(format!("new_global_ref context: {e}")))?
    };
    Ok((vm, global))
}

pub mod jni_api {
    use super::{DeepLinkError, JNIEnv, JObject, JString, JValue};

    /// Check whether a URL can be opened with the given Android context.
    ///
    /// # Errors
    /// Returns an error if JNI calls fail.
    pub fn can_open_url_with_context(
        env: &mut JNIEnv,
        context: &JObject,
        url: &str,
    ) -> Result<bool, DeepLinkError> {
        let uri_class = env
            .find_class("android/net/Uri")
            .map_err(|e| DeepLinkError::PlatformError(format!("find Uri: {e}")))?;
        let intent_class = env
            .find_class("android/content/Intent")
            .map_err(|e| DeepLinkError::PlatformError(format!("find Intent: {e}")))?;

        let action_view = env
            .new_string("android.intent.action.VIEW")
            .map_err(|e| DeepLinkError::PlatformError(format!("new action string: {e}")))?;
        let url_j = env
            .new_string(url)
            .map_err(|e| DeepLinkError::PlatformError(format!("new url string: {e}")))?;

        let uri = env
            .call_static_method(
                uri_class,
                "parse",
                "(Ljava/lang/String;)Landroid/net/Uri;",
                &[JValue::Object(&url_j)],
            )
            .and_then(jni::objects::JValueGen::l)
            .map_err(|e| DeepLinkError::PlatformError(format!("Uri.parse: {e}")))?;

        let intent = env
            .new_object(
                intent_class,
                "(Ljava/lang/String;Landroid/net/Uri;)V",
                &[JValue::Object(&action_view), JValue::Object(&uri)],
            )
            .map_err(|e| DeepLinkError::PlatformError(format!("new Intent: {e}")))?;

        let package_manager = env
            .call_method(
                context,
                "getPackageManager",
                "()Landroid/content/pm/PackageManager;",
                &[],
            )
            .and_then(jni::objects::JValueGen::l)
            .map_err(|e| DeepLinkError::PlatformError(format!("getPackageManager: {e}")))?;

        let resolved = env
            .call_method(
                &package_manager,
                "resolveActivity",
                "(Landroid/content/Intent;I)Landroid/content/pm/ResolveInfo;",
                &[JValue::Object(&intent), JValue::Int(0)],
            )
            .and_then(jni::objects::JValueGen::l)
            .map_err(|e| DeepLinkError::PlatformError(format!("resolveActivity: {e}")))?;

        Ok(!resolved.is_null())
    }

    /// Read the deeplink URL from the current activity intent.
    ///
    /// # Errors
    /// Returns an error if JNI calls fail.
    pub fn intent_url_with_context(
        env: &mut JNIEnv,
        context: &JObject,
    ) -> Result<Option<String>, DeepLinkError> {
        let intent = env
            .call_method(context, "getIntent", "()Landroid/content/Intent;", &[])
            .and_then(jni::objects::JValueGen::l)
            .map_err(|e| DeepLinkError::PlatformError(format!("getIntent: {e}")))?;
        if intent.is_null() {
            return Ok(None);
        }

        let data = env
            .call_method(&intent, "getDataString", "()Ljava/lang/String;", &[])
            .and_then(jni::objects::JValueGen::l)
            .map_err(|e| DeepLinkError::PlatformError(format!("getDataString: {e}")))?;
        if data.is_null() {
            return Ok(None);
        }

        let data_string = JString::from(data);
        let value = env
            .get_string(&data_string)
            .map_err(|e| DeepLinkError::PlatformError(format!("get_string dataString: {e}")))?;
        Ok(Some(value.into()))
    }

    /// Open a URL with the given Android context.
    ///
    /// # Errors
    /// Returns an error if JNI calls fail.
    pub fn open_url_with_context(
        env: &mut JNIEnv,
        context: &JObject,
        url: &str,
    ) -> Result<(), DeepLinkError> {
        let uri_class = env
            .find_class("android/net/Uri")
            .map_err(|e| DeepLinkError::PlatformError(format!("find Uri: {e}")))?;
        let intent_class = env
            .find_class("android/content/Intent")
            .map_err(|e| DeepLinkError::PlatformError(format!("find Intent: {e}")))?;

        let action_view = env
            .new_string("android.intent.action.VIEW")
            .map_err(|e| DeepLinkError::PlatformError(format!("new action string: {e}")))?;
        let url_j = env
            .new_string(url)
            .map_err(|e| DeepLinkError::PlatformError(format!("new url string: {e}")))?;

        let uri = env
            .call_static_method(
                uri_class,
                "parse",
                "(Ljava/lang/String;)Landroid/net/Uri;",
                &[JValue::Object(&url_j)],
            )
            .and_then(jni::objects::JValueGen::l)
            .map_err(|e| DeepLinkError::PlatformError(format!("Uri.parse: {e}")))?;

        let intent = env
            .new_object(
                intent_class,
                "(Ljava/lang/String;Landroid/net/Uri;)V",
                &[JValue::Object(&action_view), JValue::Object(&uri)],
            )
            .map_err(|e| DeepLinkError::PlatformError(format!("new Intent: {e}")))?;

        let flag = 0x1000_0000_i32;
        env.call_method(
            &intent,
            "addFlags",
            "(I)Landroid/content/Intent;",
            &[JValue::Int(flag)],
        )
        .map_err(|e| DeepLinkError::PlatformError(format!("addFlags: {e}")))?;

        env.call_method(
            context,
            "startActivity",
            "(Landroid/content/Intent;)V",
            &[JValue::Object(&intent)],
        )
        .map_err(|e| DeepLinkError::PlatformError(format!("startActivity: {e}")))?;

        Ok(())
    }
}

#[allow(clippy::unused_async)]
pub async fn open_url(url: &str) -> Result<(), DeepLinkError> {
    let (vm, context) = ensure_context_global()?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| DeepLinkError::PlatformError(format!("attach_current_thread: {e}")))?;
    jni_api::open_url_with_context(&mut env, context.as_obj(), url)
}

#[allow(clippy::unused_async)]
pub async fn can_open_url(url: &str) -> Result<bool, DeepLinkError> {
    let (vm, context) = ensure_context_global()?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| DeepLinkError::PlatformError(format!("attach_current_thread: {e}")))?;
    jni_api::can_open_url_with_context(&mut env, context.as_obj(), url)
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
    #[allow(clippy::unused_async)]
    pub async fn start() -> Result<(Self, async_channel::Receiver<DeepLink>), DeepLinkError> {
        let (vm, context) = ensure_context_global()?;
        let (link_tx, link_rx) = async_channel::bounded(16);
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop_flag);

        let worker = std::thread::Builder::new()
            .name("waterkit-deeplink-android".to_owned())
            .spawn(move || {
                let mut last_url: Option<String> = None;
                while !stop_for_thread.load(Ordering::Relaxed) {
                    let Ok(mut env) = vm.attach_current_thread() else {
                        std::thread::sleep(Duration::from_millis(250));
                        continue;
                    };

                    if let Ok(Some(url)) =
                        jni_api::intent_url_with_context(&mut env, context.as_obj())
                        && last_url.as_deref() != Some(url.as_str())
                    {
                        if let Ok(link) = DeepLink::parse(&url) {
                            let _ = link_tx.try_send(link);
                        }
                        last_url = Some(url);
                    }

                    std::thread::sleep(Duration::from_millis(250));
                }
            })
            .map_err(|e| DeepLinkError::PlatformError(format!("spawn deeplink listener: {e}")))?;

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
        let (vm, context) = ensure_context_global()?;
        let mut env = vm
            .attach_current_thread()
            .map_err(|e| DeepLinkError::PlatformError(format!("attach_current_thread: {e}")))?;
        jni_api::intent_url_with_context(&mut env, context.as_obj())?
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
