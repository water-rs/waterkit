//! Android clipboard implementation using JNI and `ndk_context`.

use crate::content::{ClipboardEvent, Image};
use crate::error::ClipboardError;
use jni::JNIEnv;
use jni::objects::{GlobalRef, JByteArray, JObject, JString, JValue};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));
static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();
type ClipboardPresence = (bool, bool, bool, bool);

/// Get JNI environment and context from `ndk_context`.
/// Panics if Android context is not available.
fn get_jni_env_and_context() -> (jni::JavaVM, GlobalRef) {
    let ctx = ndk_context::android_context();
    let vm = unsafe {
        jni::JavaVM::from_raw(ctx.vm().cast()).expect("Failed to get JavaVM from ndk_context")
    };

    let context = unsafe {
        let env = vm
            .attach_current_thread()
            .expect("Failed to attach JNI thread");
        env.new_global_ref(JObject::from_raw(ctx.context().cast()))
            .expect("Failed to create global ref for context")
    };

    (vm, context)
}

/// Initialize the clipboard helper DEX.
fn init_dex(env: &mut JNIEnv, context: &JObject) -> Result<(), ClipboardError> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }

    // Standard DEX loading boilerplate
    let cache_dir = env
        .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| ClipboardError::Platform(format!("JNI error getCacheDir: {e}")))?;

    let cache_path = env
        .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| ClipboardError::Platform(format!("JNI error getAbsolutePath: {e}")))?;

    let dex_path = format!(
        "{}/waterkit_clipboard.dex",
        env.get_string((&cache_path).into())
            .map_err(|e| ClipboardError::Platform(format!("JNI error get_string: {e}")))?
            .to_str()
            .map_err(|e| ClipboardError::Platform(format!("JNI error to_str: {e}")))?
    );

    std::fs::write(&dex_path, DEX_BYTES)
        .map_err(|e| ClipboardError::Platform(format!("Failed to write DEX: {e}")))?;

    let dex_path_jstring = env
        .new_string(&dex_path)
        .map_err(|e| ClipboardError::Platform(format!("JNI error new_string: {e}")))?;

    let parent_loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| ClipboardError::Platform(format!("JNI error getClassLoader: {e}")))?;

    let dex_class_loader_class = env
        .find_class("dalvik/system/DexClassLoader")
        .map_err(|e| ClipboardError::Platform(format!("JNI error find_class: {e}")))?;

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
        .map_err(|e| ClipboardError::Platform(format!("JNI error new_object: {e}")))?;

    let global_ref = env
        .new_global_ref(class_loader)
        .map_err(|e| ClipboardError::Platform(format!("JNI error new_global_ref: {e}")))?;

    let _ = CLASS_LOADER.set(global_ref);
    Ok(())
}

fn query_clipboard_presence(env: &mut JNIEnv, context: &GlobalRef) -> ClipboardPresence {
    let Ok(helper_class) = get_helper_class(env) else {
        return (false, false, false, false);
    };
    let context = context.as_obj();
    let has_text = env
        .call_static_method(
            &helper_class,
            "hasText",
            "(Landroid/content/Context;)Z",
            &[JValue::Object(context)],
        )
        .and_then(jni::objects::JValueGen::z)
        .unwrap_or(false);
    let has_html = env
        .call_static_method(
            &helper_class,
            "hasHtml",
            "(Landroid/content/Context;)Z",
            &[JValue::Object(context)],
        )
        .and_then(jni::objects::JValueGen::z)
        .unwrap_or(false);
    let has_files = env
        .call_static_method(
            &helper_class,
            "hasFiles",
            "(Landroid/content/Context;)Z",
            &[JValue::Object(context)],
        )
        .and_then(jni::objects::JValueGen::z)
        .unwrap_or(false);
    let has_image = env
        .call_static_method(
            &helper_class,
            "hasImage",
            "(Landroid/content/Context;)Z",
            &[JValue::Object(context)],
        )
        .and_then(jni::objects::JValueGen::z)
        .unwrap_or(false);
    (has_text, has_html, has_files, has_image)
}

fn get_helper_class<'a>(env: &mut JNIEnv<'a>) -> Result<jni::objects::JClass<'a>, ClipboardError> {
    let class_loader = CLASS_LOADER.get().ok_or(ClipboardError::Unavailable)?;

    let helper_class_name = env
        .new_string("waterkit/clipboard/ClipboardHelper")
        .map_err(|e| ClipboardError::Platform(format!("JNI error new_string name: {e}")))?;

    let helper_class = env
        .call_method(
            class_loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&helper_class_name)],
        )
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| ClipboardError::Platform(format!("JNI error loadClass: {e}")))?;

    Ok(helper_class.into())
}

/// Android clipboard handle.
#[derive(Debug)]
pub struct ClipboardInner {
    vm: jni::JavaVM,
    context: GlobalRef,
}

impl ClipboardInner {
    /// Create a new clipboard handle.
    ///
    /// # Panics
    ///
    /// Panics if Android context is not available via `ndk_context`.
    pub fn new() -> Result<Self, ClipboardError> {
        let (vm, context) = get_jni_env_and_context();

        // Initialize DEX
        {
            let mut env = vm
                .attach_current_thread()
                .map_err(|e| ClipboardError::Platform(format!("JNI attach error: {e}")))?;
            init_dex(&mut env, context.as_obj())?;
        }

        Ok(Self { vm, context })
    }

    fn with_env<T, F>(&self, f: F) -> Result<T, ClipboardError>
    where
        F: FnOnce(&mut JNIEnv, &JObject) -> Result<T, ClipboardError>,
    {
        let mut env = self
            .vm
            .attach_current_thread()
            .map_err(|e| ClipboardError::Platform(format!("JNI attach error: {e}")))?;
        f(&mut env, self.context.as_obj())
    }

    // ========== Query (sync) ==========

    /// Check if text is available.
    pub fn has_text(&self) -> bool {
        self.with_env(|env, context| {
            let helper_class = get_helper_class(env)?;
            Ok(env
                .call_static_method(
                    &helper_class,
                    "hasText",
                    "(Landroid/content/Context;)Z",
                    &[JValue::Object(context)],
                )
                .and_then(jni::objects::JValueGen::z)
                .unwrap_or(false))
        })
        .unwrap_or(false)
    }

    /// Check if HTML is available.
    pub fn has_html(&self) -> bool {
        self.with_env(|env, context| {
            let helper_class = get_helper_class(env)?;
            Ok(env
                .call_static_method(
                    &helper_class,
                    "hasHtml",
                    "(Landroid/content/Context;)Z",
                    &[JValue::Object(context)],
                )
                .and_then(jni::objects::JValueGen::z)
                .unwrap_or(false))
        })
        .unwrap_or(false)
    }

    /// Check if files are available.
    pub fn has_files(&self) -> bool {
        self.with_env(|env, context| {
            let helper_class = get_helper_class(env)?;
            Ok(env
                .call_static_method(
                    &helper_class,
                    "hasFiles",
                    "(Landroid/content/Context;)Z",
                    &[JValue::Object(context)],
                )
                .and_then(jni::objects::JValueGen::z)
                .unwrap_or(false))
        })
        .unwrap_or(false)
    }

    /// Check if image is available.
    pub fn has_image(&self) -> bool {
        self.with_env(|env, context| {
            let helper_class = get_helper_class(env)?;
            Ok(env
                .call_static_method(
                    &helper_class,
                    "hasImage",
                    "(Landroid/content/Context;)Z",
                    &[JValue::Object(context)],
                )
                .and_then(jni::objects::JValueGen::z)
                .unwrap_or(false))
        })
        .unwrap_or(false)
    }

    // ========== Read (sync, called from blocking::unblock) ==========

    /// Get text content.
    pub fn get_text(&self) -> Result<Option<String>, ClipboardError> {
        self.with_env(|env, context| {
            let helper_class = get_helper_class(env)?;

            let result = env
                .call_static_method(
                    helper_class,
                    "getText",
                    "(Landroid/content/Context;)Ljava/lang/String;",
                    &[JValue::Object(context)],
                )
                .map_err(|e| ClipboardError::Platform(format!("JNI error getText: {e}")))?;

            let obj = result
                .l()
                .map_err(|e| ClipboardError::Platform(format!("JNI error result: {e}")))?;

            if obj.is_null() {
                Ok(None)
            } else {
                let jstring = unsafe { JString::from_raw(obj.into_raw()) };
                let text = env
                    .get_string(&jstring)
                    .map_err(|e| ClipboardError::Platform(format!("JNI error get_string: {e}")))?;
                Ok(Some(text.into()))
            }
        })
    }

    /// Get HTML content.
    pub fn get_html(&self) -> Result<Option<String>, ClipboardError> {
        self.with_env(|env, context| {
            let helper_class = get_helper_class(env)?;

            let result = env
                .call_static_method(
                    helper_class,
                    "getHtml",
                    "(Landroid/content/Context;)Ljava/lang/String;",
                    &[JValue::Object(context)],
                )
                .map_err(|e| ClipboardError::Platform(format!("JNI error getHtml: {e}")))?;

            let obj = result
                .l()
                .map_err(|e| ClipboardError::Platform(format!("JNI error result: {e}")))?;

            if obj.is_null() {
                Ok(None)
            } else {
                let jstring = unsafe { JString::from_raw(obj.into_raw()) };
                let text = env
                    .get_string(&jstring)
                    .map_err(|e| ClipboardError::Platform(format!("JNI error get_string: {e}")))?;
                Ok(Some(text.into()))
            }
        })
    }

    /// Get file paths.
    pub fn get_files(&self) -> Result<Vec<PathBuf>, ClipboardError> {
        self.with_env(|env, context| {
            let helper_class = get_helper_class(env)?;

            let result = env
                .call_static_method(
                    helper_class,
                    "getFileUri",
                    "(Landroid/content/Context;)Ljava/lang/String;",
                    &[JValue::Object(context)],
                )
                .map_err(|e| ClipboardError::Platform(format!("JNI error getFileUri: {e}")))?;

            let obj = result
                .l()
                .map_err(|e| ClipboardError::Platform(format!("JNI error result: {e}")))?;

            if obj.is_null() {
                return Ok(Vec::new());
            }

            let jstring = unsafe { JString::from_raw(obj.into_raw()) };
            let url: String = env
                .get_string(&jstring)
                .map_err(|e| ClipboardError::Platform(format!("JNI error get_string: {e}")))?
                .into();

            if let Some(path) = url.strip_prefix("file://") {
                let decoded = percent_encoding::percent_decode_str(path)
                    .decode_utf8()
                    .map_err(|e| ClipboardError::Platform(format!("Invalid URL encoding: {e}")))?;
                Ok(vec![PathBuf::from(decoded.into_owned())])
            } else {
                Ok(Vec::new())
            }
        })
    }

    /// Get image as RGBA.
    pub fn get_image(&self) -> Result<Option<Image>, ClipboardError> {
        self.with_env(|env, context| {
            let helper_class = get_helper_class(env)?;

            // Get width
            let width = env
                .call_static_method(
                    &helper_class,
                    "getImageWidth",
                    "(Landroid/content/Context;)I",
                    &[JValue::Object(context)],
                )
                .and_then(jni::objects::JValueGen::i)
                .unwrap_or(-1);

            if width <= 0 {
                return Ok(None);
            }

            // Get height
            let height = env
                .call_static_method(
                    &helper_class,
                    "getImageHeight",
                    "(Landroid/content/Context;)I",
                    &[JValue::Object(context)],
                )
                .and_then(jni::objects::JValueGen::i)
                .unwrap_or(-1);

            if height <= 0 {
                return Ok(None);
            }

            // Get RGBA bytes
            let bytes_result = env
                .call_static_method(
                    &helper_class,
                    "getImageRgba",
                    "(Landroid/content/Context;)[B",
                    &[JValue::Object(context)],
                )
                .map_err(|e| ClipboardError::Platform(format!("JNI error getImageRgba: {e}")))?;

            let obj = bytes_result
                .l()
                .map_err(|e| ClipboardError::Platform(format!("JNI error result: {e}")))?;

            if obj.is_null() {
                return Ok(None);
            }

            let byte_array = unsafe { JByteArray::from_raw(obj.into_raw()) };
            let bytes = env.convert_byte_array(&byte_array).map_err(|e| {
                ClipboardError::Platform(format!("JNI error convert_byte_array: {e}"))
            })?;

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
            let helper_class = get_helper_class(env)?;

            let jmime = env
                .new_string(&mime)
                .map_err(|e| ClipboardError::Platform(format!("JNI error new_string: {e}")))?;

            let result = env
                .call_static_method(
                    helper_class,
                    "getBinary",
                    "(Landroid/content/Context;Ljava/lang/String;)[B",
                    &[JValue::Object(context), JValue::Object(&jmime)],
                )
                .map_err(|e| ClipboardError::Platform(format!("JNI error getBinary: {e}")))?;

            let obj = result
                .l()
                .map_err(|e| ClipboardError::Platform(format!("JNI error result: {e}")))?;

            if obj.is_null() {
                return Ok(None);
            }

            let byte_array = unsafe { JByteArray::from_raw(obj.into_raw()) };
            let bytes = env.convert_byte_array(&byte_array).map_err(|e| {
                ClipboardError::Platform(format!("JNI error convert_byte_array: {e}"))
            })?;

            Ok(Some(bytes))
        })
    }

    // ========== Write (sync) ==========

    /// Set text content.
    pub fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        let text = text.to_string();
        self.with_env(|env, context| {
            let helper_class = get_helper_class(env)?;

            let jtext = env
                .new_string(&text)
                .map_err(|e| ClipboardError::Platform(format!("JNI error new_string: {e}")))?;

            env.call_static_method(
                helper_class,
                "setText",
                "(Landroid/content/Context;Ljava/lang/String;)V",
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
            let helper_class = get_helper_class(env)?;

            let jhtml = env
                .new_string(&html)
                .map_err(|e| ClipboardError::Platform(format!("JNI error new_string html: {e}")))?;
            let jalt = env
                .new_string(&alt)
                .map_err(|e| ClipboardError::Platform(format!("JNI error new_string alt: {e}")))?;

            env.call_static_method(
                helper_class,
                "setHtml",
                "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)V",
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
            let helper_class = get_helper_class(env)?;

            let juri = env
                .new_string(&url)
                .map_err(|e| ClipboardError::Platform(format!("JNI error new_string: {e}")))?;

            env.call_static_method(
                helper_class,
                "setFileUri",
                "(Landroid/content/Context;Ljava/lang/String;)V",
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
            let helper_class = get_helper_class(env)?;

            let jpath = env
                .new_string(&path_str)
                .map_err(|e| ClipboardError::Platform(format!("JNI error new_string: {e}")))?;

            let success = env
                .call_static_method(
                    helper_class,
                    "setImageFromPath",
                    "(Landroid/content/Context;Ljava/lang/String;)Z",
                    &[JValue::Object(context), JValue::Object(&jpath)],
                )
                .and_then(jni::objects::JValueGen::z)
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
            let helper_class = get_helper_class(env)?;

            let jdata = env
                .byte_array_from_slice(&data)
                .map_err(|e| ClipboardError::Platform(format!("JNI error byte_array: {e}")))?;
            let jmime = env
                .new_string(&mime)
                .map_err(|e| ClipboardError::Platform(format!("JNI error new_string: {e}")))?;

            env.call_static_method(
                helper_class,
                "setBinary",
                "(Landroid/content/Context;[BLjava/lang/String;)V",
                &[
                    JValue::Object(context),
                    JValue::Object(&jdata),
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
            let helper_class = get_helper_class(env)?;

            env.call_static_method(
                helper_class,
                "clear",
                "(Landroid/content/Context;)V",
                &[JValue::Object(context)],
            )
            .map_err(|e| ClipboardError::Platform(format!("JNI error clear: {e}")))?;

            Ok(())
        })
    }
}

/// Start watching clipboard changes.
///
/// Uses polling since Android doesn't have a native clipboard change listener
/// that works across all API levels.
/// Returns a receiver and a stop flag.
pub fn start_watch() -> (async_channel::Receiver<ClipboardEvent>, Arc<AtomicBool>) {
    let (sender, receiver) = async_channel::unbounded();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = Arc::clone(&stop);

    // Get context for the watcher thread
    let (vm, context) = get_jni_env_and_context();

    thread::spawn(move || {
        let mut last_has_text = false;
        let mut last_has_html = false;
        let mut last_has_files = false;
        let mut last_has_image = false;

        while !stop_clone.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(500));

            // Check clipboard types
            let (has_text, has_html, has_files, has_image) = vm
                .attach_current_thread()
                .map_or((false, false, false, false), |mut env| {
                    query_clipboard_presence(&mut env, &context)
                });

            // Only send event if types changed
            if has_text != last_has_text
                || has_html != last_has_html
                || has_files != last_has_files
                || has_image != last_has_image
            {
                last_has_text = has_text;
                last_has_html = has_html;
                last_has_files = has_files;
                last_has_image = has_image;

                let event = ClipboardEvent::new(has_text, has_html, has_files, has_image);
                if sender.try_send(event).is_err() {
                    break;
                }
            }
        }
    });

    (receiver, stop)
}
