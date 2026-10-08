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
use waterkit_build::describe_jni_error;

use super::mlkit::{BARCODE_HELPER, MlInput, SharedInput, on_vision_thread, prepare_module, quad};
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

/// Reads one `BarcodeRow` into a [`Barcode`].
fn barcode_row(
    env: &mut Env<'_>,
    row: &JObject<'_>,
    input: &MlInput,
) -> Result<Barcode, VisionError> {
    let format = env
        .get_field(row, jni_str!("format"), jni_sig!("I"))
        .and_then(JValueOwned::i)
        .map_err(|error| VisionError::Platform(format!("barcode format: {error}")))?;
    let symbology = symbology_of(format).ok_or_else(|| {
        VisionError::Platform(format!("barcode reported unknown format {format}"))
    })?;
    let bytes = env
        .get_field(row, jni_str!("bytes"), jni_sig!("[B"))
        .and_then(JValueOwned::l)
        .map_err(|error| VisionError::Platform(format!("barcode bytes: {error}")))?;
    let bytes = env
        .cast_local::<JByteArray>(bytes)
        .map_err(|error| VisionError::Platform(format!("barcode bytes cast: {error}")))?;
    let bytes = env
        .convert_byte_array(&bytes)
        .map_err(|error| VisionError::Platform(format!("barcode bytes read: {error}")))?;
    let points = env
        .get_field(row, jni_str!("points"), jni_sig!("[I"))
        .and_then(JValueOwned::l)
        .map_err(|error| VisionError::Platform(format!("barcode points: {error}")))?;
    let points = env
        .cast_local::<JIntArray>(points)
        .map_err(|error| VisionError::Platform(format!("barcode points cast: {error}")))?;
    let length = points
        .len(env)
        .map_err(|error| VisionError::Platform(format!("barcode points len: {error}")))?;
    let mut flat = vec![0i32; length];
    points
        .get_region(env, 0, &mut flat)
        .map_err(|error| VisionError::Platform(format!("barcode points read: {error}")))?;
    Ok(Barcode {
        symbology,
        payload: Payload {
            bytes: Bytes::from(bytes),
        },
        bounds: quad(input.rotation_degrees, input.width, input.height, &flat),
    })
}

/// Runs `symbologies` over the pass's shared [`MlInput`], installing the
/// barcode module on the worker thread when Play services lacks it.
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
    on_vision_thread("waterkit-vision-barcode", move |env, context| {
        prepare_module(env, context, &BARCODE_HELPER, 0)?;
        let class = BARCODE_HELPER.class(env, context)?;
        let formats_array = JIntArray::new(env, formats.len())
            .map_err(|error| VisionError::Platform(format!("formats array: {error}")))?;
        formats_array
            .set_region(env, 0, &formats)
            .map_err(|error| VisionError::Platform(format!("formats array: {error}")))?;
        let rows = env
            .call_static_method(
                class,
                jni_str!("detectBarcodes"),
                jni_sig!(
                    "(Lcom/google/mlkit/vision/common/InputImage;[I)[Lwaterkit/vision/BarcodeHelper$BarcodeRow;"
                ),
                &[
                    JValue::Object(input.input.as_obj()),
                    JValue::Object(&formats_array),
                ],
            )
            .and_then(JValueOwned::l)
            .map_err(|error| {
                VisionError::Platform(format!(
                    "detect barcodes: {}",
                    describe_jni_error(env, error)
                ))
            })?;
        let rows = env
            .cast_local::<JObjectArray<JObject>>(rows)
            .map_err(|error| VisionError::Platform(format!("barcode rows cast: {error}")))?;
        let count = rows
            .len(env)
            .map_err(|error| VisionError::Platform(format!("barcode rows len: {error}")))?;
        let mut barcodes = Vec::with_capacity(count);
        for index in 0..count {
            let row = rows.get_element(env, index).map_err(|error| {
                VisionError::Platform(format!("barcode row {index}: {error}"))
            })?;
            barcodes.push(barcode_row(env, &row, &input)?);
        }
        Ok(barcodes)
    })
    .await
}
