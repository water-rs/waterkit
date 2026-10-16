//! The Google code scanner of Google Play services.
//!
//! `waterkit.vision.ScannerHelper` is compiled into the application's
//! classpath by the packager together with the thin
//! `play-services-code-scanner` client this crate declares. The scanning
//! screen itself is Play services' module, so the app needs no camera
//! permission. `startScan` answers on a `Task`, so nothing blocks: the
//! Kotlin listener delivers the outcome through a `NativeCallback` the
//! helper receives as an argument, and this side completes the awaiting
//! [`crate::CodeScanner::scan`] through its oneshot.

use crate::sys::android::{format_of, served_symbologies, symbology_of};
use crate::{Payload, ScannedCode, Symbology, VisionError};
use bytes::Bytes;
use enumset::EnumSet;
use futures::channel::oneshot;
use jni::objects::{JByteArray, JObject, JValue, JValueOwned};
use jni::{Env, jni_sig, jni_str};
use waterkit_build::{
    AndroidError, DexHelper, FromJava, NativeCallback, PeerError, describe_jni_error, dex_helper,
    with_android_context,
};

/// `waterkit.vision.ScannerHelper`, compiled into the app's DEX by the
/// packager and resolved through the application's `ClassLoader`.
static HELPER: DexHelper = dex_helper!("waterkit.vision.ScannerHelper");

/// The Google code scanner's format constant for `symbology`.
fn gms_format(symbology: Symbology) -> i32 {
    format_of(symbology)
        .expect("waterkit-vision: the Google code scanner cannot express this symbology")
}

fn symbology_from_gms_format(format: i32) -> Symbology {
    symbology_of(format).unwrap_or_else(|| {
        panic!("waterkit-vision: the scanner returned an unknown format {format}")
    })
}

/// What the Kotlin helper completes the scan's `NativeCallback` with: the
/// ML Kit `Barcode` on success, `null` when the user dismissed the scanner.
struct ScanOutcome(Option<ScannedCode>);

impl FromJava for ScanOutcome {
    fn from_java(env: &mut Env<'_>, object: &JObject<'_>) -> Result<Self, AndroidError> {
        if object.is_null() {
            return Ok(Self(None));
        }
        let bytes = env
            .call_method(object, jni_str!("getRawBytes"), jni_sig!("()[B"), &[])
            .and_then(JValueOwned::l)?;
        let bytes = env.convert_byte_array(&env.cast_local::<JByteArray>(bytes)?)?;
        let format = env
            .call_method(object, jni_str!("getFormat"), jni_sig!("()I"), &[])
            .and_then(JValueOwned::i)?;
        Ok(Self(Some(ScannedCode {
            symbology: symbology_from_gms_format(format),
            payload: Payload {
                bytes: Bytes::from(bytes),
            },
        })))
    }
}

/// The symbologies the Google code scanner can restrict a scan to.
pub fn scanner_symbologies() -> EnumSet<Symbology> {
    served_symbologies()
}

/// Whether Google Play services is usable on this device — the device's own
/// answer, which makes the system code scanner available.
///
/// # Panics
///
/// Panics if `ndk_context` has no `JavaVM` or Android `Context` yet, or the
/// JNI probe fails.
pub fn scanner_available() -> bool {
    crate::sys::android::play_services().unwrap_or_else(|error| panic!("waterkit-vision: {error}"))
}

fn launch_scan_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    symbologies: EnumSet<Symbology>,
) -> Result<oneshot::Receiver<Result<ScanOutcome, PeerError>>, VisionError> {
    let helper_class = HELPER.class(env, context)?;

    let formats: Vec<i32> = symbologies.iter().map(gms_format).collect();
    let formats_array = env
        .new_int_array(formats.len())
        .and_then(|array| {
            array.set_region(env, 0, &formats)?;
            Ok(array)
        })
        .map_err(|error| VisionError::Platform(format!("build scan formats failed: {error}")))?;

    let (callback, rx) = NativeCallback::<ScanOutcome>::new(env).map_err(|error| {
        VisionError::Platform(format!("create the scan callback failed: {error}"))
    })?;

    env.call_static_method(
        helper_class,
        jni_str!("scan"),
        jni_sig!("(Landroid/content/Context;Lwaterkit/build/NativeCallback;[I)V"),
        &[
            JValue::Object(context),
            JValue::Object(callback.as_obj()),
            JValue::Object(&formats_array),
        ],
    )
    .map_err(|error| {
        VisionError::Platform(format!(
            "launch the system code scanner failed: {}",
            describe_jni_error(env, error)
        ))
    })?;
    Ok(rx)
}

/// Presents the Google code scanner and resolves to the scanned barcode.
///
/// # Errors
///
/// Returns [`VisionError::Unsupported`] when the device has no usable Google
/// Play services and [`VisionError::Platform`] when JNI or the scanner task
/// fails.
///
/// # Panics
///
/// Panics if `ndk_context` has no `JavaVM` or Android `Context` yet.
pub async fn scan(symbologies: EnumSet<Symbology>) -> Result<Option<ScannedCode>, VisionError> {
    if !scanner_available() {
        return Err(VisionError::Unsupported(
            "Google Play services is unavailable, so this device has no system code scanner"
                .to_owned(),
        ));
    }
    let rx =
        with_android_context(|env, context| launch_scan_with_context(env, context, symbologies))?;
    let outcome = rx
        .await
        .map_err(|_| {
            VisionError::Platform("the scan callback was collected unanswered".to_owned())
        })?
        .map_err(|error| VisionError::Platform(error.to_string()))?;
    Ok(outcome.0)
}
