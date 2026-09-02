//! Android clipboard implementation using JNI and `ndk_context`.

use crate::content::{ClipboardEvent, Image};
use crate::error::ClipboardError;
use jni::objects::{Global, JByteArray, JObject, JValue};
use jni::{Env, JavaVM, jni_sig, jni_str};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;
use waterkit_build::{AndroidError, DexHelper, decode_string, dex_helper, jvm_and_context};

/// `waterkit.clipboard.ClipboardHelper`, embedded as a DEX by this crate's build script and
/// loaded on first use.
static HELPER: DexHelper = dex_helper!("waterkit.clipboard.ClipboardHelper");

impl From<AndroidError> for ClipboardError {
    fn from(error: AndroidError) -> Self {
        Self::Platform(error.to_string())
    }
}

type ClipboardPresence = (bool, bool, bool, bool);

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

fn query_clipboard_presence(env: &mut Env<'_>, context: &JObject<'_>) -> ClipboardPresence {
    (
        probe(env, context, jni_str!("hasText")),
        probe(env, context, jni_str!("hasHtml")),
        probe(env, context, jni_str!("hasFiles")),
        probe(env, context, jni_str!("hasImage")),
    )
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

        // Load the helper DEX up front so later calls are plain lookups.
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

/// Start watching clipboard changes.
///
/// Uses polling since Android doesn't have a native clipboard change listener
/// that works across all API levels.
/// Returns a receiver and a stop flag.
///
/// # Panics
///
/// Panics if the Android context is not available via `ndk_context`.
pub fn start_watch() -> (async_channel::Receiver<ClipboardEvent>, Arc<AtomicBool>) {
    let (sender, receiver) = async_channel::unbounded();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = Arc::clone(&stop);

    // Get context for the watcher thread
    let (vm, context) = jvm_and_context().unwrap_or_else(|error| {
        panic!("waterkit-clipboard: failed to resolve the Android context for watching: {error}")
    });

    thread::spawn(move || {
        let _ = vm.attach_current_thread(|env| -> jni::errors::Result<()> {
            let mut last_has_text = false;
            let mut last_has_html = false;
            let mut last_has_files = false;
            let mut last_has_image = false;

            while !stop_clone.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(500));

                let (has_text, has_html, has_files, has_image) =
                    query_clipboard_presence(env, context.as_obj());

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
            Ok(())
        });
    });

    (receiver, stop)
}
