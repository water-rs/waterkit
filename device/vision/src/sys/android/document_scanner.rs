//! Android realization: the ML Kit document scanner of Google Play
//! services.
//!
//! `waterkit.vision.DocumentScannerHelper` is compiled into the
//! application's classpath by the packager together with the thin
//! `play-services-mlkit-document-scanner` client this crate declares. The
//! scanning screen itself is Play services' module, so the app needs no
//! camera permission. `getStartScanIntent` answers on a `Task` with an
//! `IntentSender`, which the `waterkit-build` activity-result bridge
//! launches on the host `ComponentActivity`; the result intent parses into
//! page URIs the helper decodes to the JPEG bytes that become the
//! [`crate::Image`] pages.

use crate::VisionError;
use crate::document_scanner::{DocumentScannerOptions, pages_from_encoded};
use bytes::Bytes;
use futures::channel::oneshot;
use jni::objects::{Global, JByteArray, JObject, JObjectArray, JValue};
use jni::sys::jboolean;
use jni::{Env, jni_sig, jni_str};
use waterkit_build::{
    AndroidError, DexHelper, FromJava, NativeCallback, PeerError, ResultCode, describe_jni_error,
    dex_helper, start_intent_sender_for_result, with_android_context,
};

/// `waterkit.vision.DocumentScannerHelper`, compiled into the app's DEX by
/// the packager and resolved through the application's `ClassLoader`.
static HELPER: DexHelper = dex_helper!("waterkit.vision.DocumentScannerHelper");

/// What the Kotlin helper completes the request's `NativeCallback` with:
/// the `IntentSender` on success, `null` when the intent task cancelled.
struct IntentOutcome(Option<Global<JObject<'static>>>);

impl FromJava for IntentOutcome {
    fn from_java(env: &mut Env<'_>, object: &JObject<'_>) -> Result<Self, AndroidError> {
        if object.is_null() {
            return Ok(Self(None));
        }
        Ok(Self(Some(env.new_global_ref(object)?)))
    }
}

/// Whether Google Play services is usable on this device — the device's own
/// answer, which makes the system document scanner available.
///
/// # Panics
///
/// Panics if `ndk_context` has no `JavaVM` or Android `Context` yet, or the
/// JNI probe fails.
pub fn document_scanner_available() -> bool {
    crate::sys::android::play_services().unwrap_or_else(|error| panic!("waterkit-vision: {error}"))
}

/// The ML Kit document scanner honors both a page limit and gallery import.
pub const fn document_scanner_options() -> DocumentScannerOptions {
    DocumentScannerOptions {
        page_limit: true,
        gallery_import: true,
    }
}

fn launch_scan_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    page_limit: Option<u16>,
    gallery_import: bool,
) -> Result<oneshot::Receiver<Result<IntentOutcome, PeerError>>, VisionError> {
    let helper_class = HELPER.class(env, context)?;

    let (callback, rx) = NativeCallback::<IntentOutcome>::new(env).map_err(|error| {
        VisionError::Platform(format!("create the document scan callback failed: {error}"))
    })?;

    // A zero page limit leaves the scanner's own default in place.
    let page_limit_jint = page_limit.map_or(0, i32::from);
    env.call_static_method(
        helper_class,
        jni_str!("scan"),
        jni_sig!("(Landroid/content/Context;Lwaterkit/build/NativeCallback;IZ)V"),
        &[
            JValue::Object(context),
            JValue::Object(callback.as_obj()),
            JValue::Int(page_limit_jint),
            JValue::Bool(jboolean::from(gallery_import)),
        ],
    )
    .map_err(|error| {
        VisionError::Platform(format!(
            "launch the system document scanner failed: {}",
            describe_jni_error(env, error)
        ))
    })?;
    Ok(rx)
}

/// Reads the scanned pages out of the scanner's result intent as JPEG
/// bytes, in scan order.
fn read_pages(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    data: &JObject<'_>,
) -> Result<Vec<Bytes>, VisionError> {
    let helper_class = HELPER.class(env, context)?;
    let array = env
        .call_static_method(
            helper_class,
            jni_str!("pageImages"),
            jni_sig!("(Landroid/content/Context;Landroid/content/Intent;)[[B"),
            &[JValue::Object(context), JValue::Object(data)],
        )
        .map_err(|error| {
            VisionError::Platform(format!(
                "read the scanned pages failed: {}",
                describe_jni_error(env, error)
            ))
        })?
        .l()
        .map_err(|error| {
            VisionError::Platform(format!("the scanned pages are not an array: {error}"))
        })?;
    let array = env
        .cast_local::<JObjectArray<JByteArray>>(array)
        .map_err(|error| {
            VisionError::Platform(format!("the scanned pages are not byte arrays: {error}"))
        })?;
    let len = array.len(env).map_err(|error| {
        VisionError::Platform(format!("count the scanned pages failed: {error}"))
    })?;
    (0..len)
        .map(|index| {
            let page = array.get_element(env, index).map_err(|error| {
                VisionError::Platform(format!("read scanned page {index} failed: {error}"))
            })?;
            env.convert_byte_array(&page)
                .map(Bytes::from)
                .map_err(|error| {
                    VisionError::Platform(format!("copy scanned page {index} failed: {error}"))
                })
        })
        .collect()
}

/// Presents the ML Kit document scanner and resolves to the scanned pages.
///
/// # Errors
///
/// Returns [`VisionError::Unsupported`] when the device has no usable Google
/// Play services and [`VisionError::Platform`] when JNI, the scanner task or
/// the activity result fails.
///
/// # Panics
///
/// Panics if `ndk_context` has no `JavaVM` or Android `Context` yet.
pub async fn scan_document(
    page_limit: Option<u16>,
    gallery_import: bool,
) -> Result<Option<Vec<crate::Image>>, VisionError> {
    if !document_scanner_available() {
        return Err(VisionError::Unsupported(
            "Google Play services is unavailable, so this device has no system document scanner"
                .to_owned(),
        ));
    }
    let rx = with_android_context(|env, context| {
        launch_scan_with_context(env, context, page_limit, gallery_import)
    })?;
    let sender = rx
        .await
        .map_err(|_| {
            VisionError::Platform("the document scan callback was collected unanswered".to_owned())
        })?
        .map_err(|error| VisionError::Platform(error.to_string()))?;
    let Some(sender) = sender.0 else {
        // `getStartScanIntent` cancelled before an intent existed.
        return Ok(None);
    };
    let pending = with_android_context(|env, _context| {
        start_intent_sender_for_result(env, sender.as_obj()).map_err(|error| {
            VisionError::Platform(format!("launch the document scanner failed: {error}"))
        })
    })?;
    let result = pending.await.map_err(|error| {
        VisionError::Platform(format!(
            "the document scanner's result failed to arrive: {error}"
        ))
    })?;
    match result.code() {
        ResultCode::Ok => {
            let data = result.into_data().ok_or_else(|| {
                VisionError::Platform("the document scanner returned no result intent".to_owned())
            })?;
            // Reading the page URIs is content-resolver I/O; keep it off the
            // async executor's thread.
            let pages = blocking::unblock(move || {
                with_android_context(|env, context| read_pages(env, context, data.as_obj()))
            })
            .await?;
            Ok(Some(pages_from_encoded(pages)))
        }
        ResultCode::Canceled => Ok(None),
        ResultCode::Custom(code) => Err(VisionError::Platform(format!(
            "the document scanner returned an unexpected result code: {code}"
        ))),
    }
}
