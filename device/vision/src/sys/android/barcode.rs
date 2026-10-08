//! Android's native barcode realization: Play services ML Kit's barcode
//! engine, unbundled.
//!
//! `play-services-mlkit-barcode-scanning` is a thin client; the engine lives
//! in a Play services module [`BARCODE_HELPER`](super::mlkit::BARCODE_HELPER)
//! installs on demand. A request's symbologies map onto `Barcode.FORMAT_*`
//! constants ([`crate::sys::android`]); one the engine cannot express
//! declines the offer, so detection only ever hands the engine formats it
//! reads.

use std::sync::Arc;

use bytes::Bytes;
use enumset::EnumSet;
use jni::objects::{JByteArray, JIntArray, JObject, JObjectArray, JValue, JValueOwned};
use jni::{Env, jni_sig, jni_str};
use waterkit_build::{
    AndroidError, FromJava, NativeCallback, describe_jni_error, with_android_context,
};

use super::mlkit::{BARCODE_HELPER, MlInput, SharedInput, prepare_module, quad};
use super::{format_of, play_services, served_symbologies, symbology_of};
use crate::{
    Barcode, Payload, Symbology, VisionError,
    sealed::{Offer, Pass},
};

/// The symbologies this device's native barcode realization serves: ML
/// Kit's format list when Google Play services is usable, none when it is
/// not.
///
/// # Panics
///
/// Panics if `ndk_context` has no `JavaVM` or Android `Context` yet, or the
/// JNI probe fails — a probe failure is a bug, not an absent realization.
pub fn supported_symbologies() -> EnumSet<Symbology> {
    match play_services() {
        Ok(true) => served_symbologies(),
        Ok(false) => EnumSet::empty(),
        Err(error) => panic!("waterkit-vision: {error}"),
    }
}

/// What the native realization offers for `requested`: every requested
/// symbology ML Kit's barcode engine expresses, or [`Offer::Lacks`] naming
/// the ones it cannot; [`Offer::Absent`] when Play services is unavailable.
pub fn barcodes_offer(requested: EnumSet<Symbology>) -> Offer {
    let missing: Vec<Symbology> = (requested - served_symbologies()).iter().collect();
    if !missing.is_empty() {
        return Offer::Lacks(format!(
            "symbologies {}",
            missing
                .iter()
                .map(|symbology| format!("{symbology:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    match play_services() {
        Ok(true) => Offer::Serves,
        Ok(false) => Offer::Absent,
        Err(error) => Offer::Lacks(error.to_string()),
    }
}

/// One `BarcodeRow` element, decoded into owned data.
struct BarcodeRow {
    format: i32,
    bytes: Vec<u8>,
    points: Vec<i32>,
}

impl BarcodeRow {
    /// The row's typed [`Barcode`]: format to symbology, points to a
    /// normalized upright quad.
    fn into_barcode(self, input: &MlInput) -> Result<Barcode, VisionError> {
        let symbology = symbology_of(self.format).ok_or_else(|| {
            VisionError::Platform(format!("barcode reported unknown format {}", self.format))
        })?;
        Ok(Barcode {
            symbology,
            payload: Payload {
                bytes: Bytes::from(self.bytes),
            },
            bounds: quad(
                input.rotation_degrees,
                input.width,
                input.height,
                &self.points,
            ),
        })
    }
}

/// What the Kotlin helper completes the request's `NativeCallback` with:
/// the `BarcodeRow[]` read field-by-field.
struct BarcodeRows(Vec<BarcodeRow>);

impl FromJava for BarcodeRows {
    fn from_java(env: &mut Env<'_>, object: &JObject<'_>) -> Result<Self, AndroidError> {
        let rows = env.as_cast::<JObjectArray<JObject>>(object)?;
        let count = rows.len(env)?;
        let mut barcodes = Vec::with_capacity(count);
        for index in 0..count {
            let row = rows.get_element(env, index)?;
            let format = env
                .get_field(&row, jni_str!("format"), jni_sig!("I"))
                .and_then(JValueOwned::i)?;
            let bytes = env
                .get_field(&row, jni_str!("bytes"), jni_sig!("[B"))
                .and_then(JValueOwned::l)?;
            let bytes = env.convert_byte_array(&env.cast_local::<JByteArray>(bytes)?)?;
            let points = env
                .get_field(&row, jni_str!("points"), jni_sig!("[I"))
                .and_then(JValueOwned::l)?;
            let points = env.cast_local::<JIntArray>(points)?;
            let mut flat = vec![0i32; points.len(env)?];
            points.get_region(env, 0, &mut flat)?;
            barcodes.push(BarcodeRow {
                format,
                bytes,
                points: flat,
            });
        }
        Ok(Self(barcodes))
    }
}

/// Runs `symbologies` over the pass's shared [`MlInput`], installing the
/// barcode module first when Play services lacks it. The module install
/// and the detection both answer through `NativeCallback`s the helper
/// completes from its task listeners, so nothing is parked waiting.
pub async fn detect_barcodes(
    pass: &mut Pass<'_>,
    symbologies: EnumSet<Symbology>,
) -> Result<Vec<Barcode>, VisionError> {
    let input = Arc::clone(&pass.prepared::<SharedInput>().await?.0);
    let formats: Vec<i32> = symbologies
        .iter()
        .map(|symbology| {
            format_of(symbology).expect("the offer rejected symbologies ML Kit cannot serve")
        })
        .collect();
    prepare_module(&BARCODE_HELPER, 0).await?;
    let rx = with_android_context(|env, context| -> Result<_, VisionError> {
        let class = BARCODE_HELPER.class(env, context)?;
        let formats_array = JIntArray::new(env, formats.len())
            .map_err(|error| VisionError::Platform(format!("formats array: {error}")))?;
        formats_array
            .set_region(env, 0, &formats)
            .map_err(|error| VisionError::Platform(format!("formats array: {error}")))?;
        let (callback, rx) = NativeCallback::<BarcodeRows>::new(env).map_err(|error| {
            VisionError::Platform(format!("create the barcode callback failed: {error}"))
        })?;
        env.call_static_method(
            class,
            jni_str!("detectBarcodes"),
            jni_sig!(
                "(Lcom/google/mlkit/vision/common/InputImage;Lwaterkit/build/NativeCallback;[I)V"
            ),
            &[
                JValue::Object(input.input.as_obj()),
                JValue::Object(callback.as_obj()),
                JValue::Object(&formats_array),
            ],
        )
        .map_err(|error| {
            VisionError::Platform(format!(
                "detect barcodes: {}",
                describe_jni_error(env, error)
            ))
        })?;
        Ok(rx)
    })?;
    let rows = rx
        .await
        .map_err(|_| {
            VisionError::Platform(String::from(
                "the barcode detection callback was collected unanswered",
            ))
        })?
        .map_err(|error| VisionError::Platform(error.to_string()))?;
    rows.0
        .into_iter()
        .map(|row| row.into_barcode(&input))
        .collect()
}
