//! Android realization: the Google code scanner of Google Play services.
//!
//! `waterkit.vision.ScannerHelper` is compiled into the application's
//! classpath by the packager together with the thin
//! `play-services-code-scanner` client this crate declares. The scanning
//! screen itself is Play services' module, so the app needs no camera
//! permission. `startScan` answers on a `Task`, so nothing blocks: the
//! Kotlin listener hands the outcome back over JNI and this side completes
//! the awaiting [`crate::CodeScanner::scan`] through a oneshot.

use crate::{Payload, ScannedCode, Symbology, VisionError};
use bytes::Bytes;
use enumset::EnumSet;
use futures::channel::oneshot;
use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{JByteArray, JClass, JObject, JValue, JValueOwned};
use jni::sys::{jint, jlong};
use jni::{Env, EnvUnowned, jni_sig, jni_str};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use waterkit_build::{
    DexHelper, decode_optional_string, describe_jni_error, dex_helper, with_android_context,
};

/// `waterkit.vision.ScannerHelper`, compiled into the app's DEX by the
/// packager and resolved through the application's `ClassLoader`.
static HELPER: DexHelper = dex_helper!("waterkit.vision.ScannerHelper");

/// The symbologies `GmsBarcodeScanning` can restrict a scan to — the
/// constants `com.google.mlkit.vision.barcode.common.Barcode#FORMAT_*`
/// defines. `Itf14` is absent: the scanner's ITF format accepts any
/// interleaved-2-of-5 length, so it cannot restrict to the 14-digit
/// symbology.
const SUPPORTED_SYMBOLOGIES: EnumSet<Symbology> = enumset::enum_set!(
    Symbology::Aztec
        | Symbology::Codabar
        | Symbology::Code39
        | Symbology::Code93
        | Symbology::Code128
        | Symbology::DataMatrix
        | Symbology::Ean8
        | Symbology::Ean13
        | Symbology::Itf
        | Symbology::Pdf417
        | Symbology::Qr
        | Symbology::UpcA
        | Symbology::UpcE
);

/// The Google code scanner's format constants
/// (`com.google.mlkit.vision.barcode.common.Barcode#FORMAT_*`).
const fn gms_format(symbology: Symbology) -> i32 {
    match symbology {
        Symbology::Aztec => 4096,
        Symbology::Codabar => 8,
        Symbology::Code39 => 2,
        Symbology::Code93 => 4,
        Symbology::Code128 => 1,
        Symbology::DataMatrix => 16,
        Symbology::Ean8 => 64,
        Symbology::Ean13 => 32,
        Symbology::Itf => 128,
        Symbology::Pdf417 => 2048,
        Symbology::Qr => 256,
        Symbology::UpcA => 512,
        Symbology::UpcE => 1024,
        _ => panic!("waterkit-vision: the Google code scanner cannot express this symbology"),
    }
}

fn symbology_from_gms_format(format: i32) -> Symbology {
    match format {
        4096 => Symbology::Aztec,
        8 => Symbology::Codabar,
        2 => Symbology::Code39,
        4 => Symbology::Code93,
        1 => Symbology::Code128,
        16 => Symbology::DataMatrix,
        64 => Symbology::Ean8,
        32 => Symbology::Ean13,
        128 => Symbology::Itf,
        2048 => Symbology::Pdf417,
        256 => Symbology::Qr,
        512 => Symbology::UpcA,
        1024 => Symbology::UpcE,
        other => panic!("waterkit-vision: the scanner returned an unknown format {other}"),
    }
}

type ScanCallback = oneshot::Sender<Result<Option<ScannedCode>, VisionError>>;

static NEXT_SCAN_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

fn scan_callbacks() -> &'static Mutex<HashMap<u64, ScanCallback>> {
    static LOCK: OnceLock<Mutex<HashMap<u64, ScanCallback>>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The symbologies the Google code scanner can restrict a scan to.
pub const fn scanner_symbologies() -> EnumSet<Symbology> {
    SUPPORTED_SYMBOLOGIES
}

/// Whether Google Play services is usable on this device — the device's own
/// answer, which makes the system code scanner available.
///
/// # Panics
///
/// Panics if `ndk_context` has no `JavaVM` or Android `Context` yet, or the
/// JNI probe fails.
pub fn scanner_available() -> bool {
    with_android_context(|env, context| {
        let helper_class = HELPER.class(env, context)?;
        env.call_static_method(
            helper_class,
            jni_str!("hasGooglePlayServices"),
            jni_sig!("(Landroid/content/Context;)Z"),
            &[JValue::Object(context)],
        )
        .and_then(JValueOwned::z)
        .map_err(|error| {
            VisionError::Platform(format!(
                "probe Google Play services failed: {}",
                describe_jni_error(env, error)
            ))
        })
    })
    .unwrap_or_else(|error| panic!("waterkit-vision: {error}"))
}

fn launch_scan_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    symbologies: EnumSet<Symbology>,
) -> Result<oneshot::Receiver<Result<Option<ScannedCode>, VisionError>>, VisionError> {
    let helper_class = HELPER.class(env, context)?;

    let formats: Vec<i32> = symbologies.iter().map(gms_format).collect();
    let formats_array = env
        .new_int_array(formats.len())
        .and_then(|array| {
            array.set_region(env, 0, &formats)?;
            Ok(array)
        })
        .map_err(|error| VisionError::Platform(format!("build scan formats failed: {error}")))?;

    let request_id = NEXT_SCAN_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    let request_id_jlong = jlong::try_from(request_id).map_err(|_| {
        VisionError::Platform(format!("scan request id exceeds jlong range: {request_id}"))
    })?;
    let (tx, rx) = oneshot::channel();
    scan_callbacks()
        .lock()
        .map_err(|error| {
            VisionError::Platform(format!("scan callback map lock poisoned: {error}"))
        })?
        .insert(request_id, tx);

    let launch_result = env.call_static_method(
        helper_class,
        jni_str!("scan"),
        jni_sig!("(Landroid/content/Context;J[I)V"),
        &[
            JValue::Object(context),
            JValue::Long(request_id_jlong),
            JValue::Object(&formats_array),
        ],
    );
    if let Err(error) = launch_result {
        scan_callbacks()
            .lock()
            .map_err(|error| {
                VisionError::Platform(format!("scan callback map lock poisoned: {error}"))
            })?
            .remove(&request_id);
        return Err(VisionError::Platform(format!(
            "launch the system code scanner failed: {}",
            describe_jni_error(env, error)
        )));
    }
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
    rx.await
        .map_err(|_| VisionError::Platform("scan result channel closed".to_owned()))?
}

// The Kotlin helper resolves its `external fun` through this symbol; the
// export attribute is the only unsafe construct a JNI bridge cannot avoid.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_waterkit_vision_ScannerHelper_onScanResult<'caller>(
    mut env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    request_id: jlong,
    payload: JObject<'caller>,
    format: jint,
    error: JObject<'caller>,
) {
    env.with_env(|env| -> jni::errors::Result<()> {
        assert!(
            request_id > 0,
            "waterkit-vision: invalid scan request id: {request_id}"
        );
        let request_id = u64::try_from(request_id).unwrap_or_else(|_| {
            panic!("waterkit-vision: scan request id conversion failed: {request_id}")
        });
        let tx = scan_callbacks()
            .lock()
            .unwrap_or_else(|error| {
                panic!("waterkit-vision: scan callback map lock poisoned: {error}")
            })
            .remove(&request_id)
            .unwrap_or_else(|| {
                panic!("waterkit-vision: unknown scan request id in callback: {request_id}")
            });

        let result = if let Some(message) =
            decode_optional_string(env, &error).unwrap_or_else(|decode_error| {
                panic!("waterkit-vision: decode scan error message failed: {decode_error}")
            }) {
            Err(VisionError::Platform(message))
        } else if payload.is_null() {
            // No error and no payload: the user dismissed the scanner.
            Ok(None)
        } else {
            let bytes = env.convert_byte_array(&env.cast_local::<JByteArray>(payload)?)?;
            Ok(Some(ScannedCode {
                symbology: symbology_from_gms_format(format),
                payload: Payload {
                    bytes: Bytes::from(bytes),
                },
            }))
        };
        let _ = tx.send(result);
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>();
}
