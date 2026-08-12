use crate::{NdefMessage, NdefRecord, NfcError, NfcTag, NfcTagType};
use jni::objects::{Global, JObject, JValue};
use jni::{Env, JavaVM};
use std::fmt::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use waterkit_build::{AndroidError, DexHelper, decode_string, dex_helper};

/// `waterkit.nfc.NfcHelper`, embedded as a DEX by this crate's build script and
/// loaded on first use.
static HELPER: DexHelper = dex_helper!("waterkit.nfc.NfcHelper");

impl From<AndroidError> for NfcError {
    fn from(error: AndroidError) -> Self {
        Self::Platform(error.to_string())
    }
}

const NFC_ACTION_TAG_DISCOVERED: &str = "android.nfc.action.TAG_DISCOVERED";
const NFC_ACTION_TECH_DISCOVERED: &str = "android.nfc.action.TECH_DISCOVERED";
const NFC_ACTION_NDEF_DISCOVERED: &str = "android.nfc.action.NDEF_DISCOVERED";
const EXTRA_TAG_KEY: &str = "android.nfc.extra.TAG";

fn with_android_context<T, F>(f: F) -> Result<T, NfcError>
where
    F: FnOnce(&mut Env<'_>, &JObject<'_>) -> Result<T, NfcError>,
{
    let android_context = ndk_context::android_context();
    let raw_vm: *mut jni::sys::JavaVM = android_context.vm().cast();
    let raw_context: jni::sys::jobject = android_context.context().cast();
    assert!(
        !raw_vm.is_null(),
        "waterkit-nfc: ndk_context returned a null JavaVM"
    );
    assert!(
        !raw_context.is_null(),
        "waterkit-nfc: ndk_context returned a null Android Context"
    );

    // SAFETY: `ndk_context` publishes the process' JavaVM pointer, which stays
    // valid for the lifetime of the application.
    let vm = unsafe { JavaVM::from_raw(raw_vm) };
    vm.attach_current_thread(|env| -> Result<Result<T, NfcError>, jni::errors::Error> {
        // SAFETY: `ndk_context` publishes a global reference to the application
        // `Context` that outlives this attachment, and `as_cast_raw` only
        // borrows it.
        let context = unsafe { env.as_cast_raw::<JObject>(&raw_context)? };
        Ok(f(env, &context))
    })
    .map_err(|e| NfcError::Platform(format!("attach_current_thread: {e}")))?
}

fn is_nfc_intent_action(action: &str) -> bool {
    matches!(
        action,
        NFC_ACTION_TAG_DISCOVERED | NFC_ACTION_TECH_DISCOVERED | NFC_ACTION_NDEF_DISCOVERED
    )
}

const fn decode_tag_type(code: i32) -> NfcTagType {
    match code {
        0 => NfcTagType::Type1,
        1 => NfcTagType::Type2,
        2 => NfcTagType::Type3,
        3 => NfcTagType::Type4,
        4 => NfcTagType::Type5,
        5 => NfcTagType::MifareClassic,
        _ => NfcTagType::Unknown,
    }
}

fn hex_decode(hex: &str) -> Result<Vec<u8>, NfcError> {
    if !hex.len().is_multiple_of(2) {
        return Err(NfcError::Platform(format!(
            "invalid hex length for NFC payload: {}",
            hex.len()
        )));
    }

    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for chunk in hex.as_bytes().as_chunks::<2>().0 {
        let value = std::str::from_utf8(chunk)
            .map_err(|e| NfcError::Platform(format!("hex utf8 decode failed: {e}")))?;
        let byte = u8::from_str_radix(value, 16)
            .map_err(|e| NfcError::Platform(format!("hex parse failed for '{value}': {e}")))?;
        bytes.push(byte);
    }
    Ok(bytes)
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut output, "{byte:02x}").expect("write to String cannot fail");
    }
    output
}

fn parse_ndef_records(records: &str) -> Result<NdefMessage, NfcError> {
    let mut parsed = Vec::new();
    for record in records.split(';').filter(|segment| !segment.is_empty()) {
        let mut parts = record.splitn(3, ':');
        let tnf = parts
            .next()
            .ok_or_else(|| NfcError::Platform("missing NDEF TNF".into()))?
            .parse::<u8>()
            .map_err(|e| NfcError::Platform(format!("invalid NDEF TNF: {e}")))?;
        let record_type = parts
            .next()
            .ok_or_else(|| NfcError::Platform("missing NDEF record type".into()))
            .and_then(hex_decode)?;
        let payload = parts
            .next()
            .ok_or_else(|| NfcError::Platform("missing NDEF payload".into()))
            .and_then(hex_decode)?;

        parsed.push(NdefRecord {
            tnf,
            record_type,
            payload,
        });
    }

    Ok(NdefMessage { records: parsed })
}

fn encode_ndef_records(message: &NdefMessage) -> String {
    message
        .records
        .iter()
        .map(|record| {
            format!(
                "{}:{}:{}",
                record.tnf,
                hex_encode(&record.record_type),
                hex_encode(&record.payload)
            )
        })
        .collect::<Vec<_>>()
        .join(";")
}

pub fn nfc_is_available() -> bool {
    with_android_context(jni_api::is_available).unwrap_or_else(|error| {
        panic!("waterkit-nfc: failed to query NFC availability with Android context: {error}")
    })
}

#[derive(Debug)]
struct TagSnapshot {
    tag: Global<JObject<'static>>,
    tag_id: String,
    tag_type: NfcTagType,
    ndef_records: Option<String>,
}

pub struct NfcReaderInner {
    latest_tag: Arc<Mutex<Option<Arc<Global<JObject<'static>>>>>>,
    stop_flag: Arc<AtomicBool>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for NfcReaderInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NfcReaderInner").finish()
    }
}

impl NfcReaderInner {
    pub async fn start_session(
        _message: &str,
    ) -> Result<(Self, async_channel::Receiver<NfcTag>), NfcError> {
        std::future::ready(()).await;
        if !nfc_is_available() {
            return Err(NfcError::NotAvailable);
        }

        let latest_tag = Arc::new(Mutex::new(None));
        let latest_tag_for_thread = Arc::clone(&latest_tag);
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop_flag);
        let (tag_tx, tag_rx) = async_channel::unbounded();

        let worker = std::thread::Builder::new()
            .name("waterkit-nfc-android".to_owned())
            .spawn(move || {
                let mut last_tag_id: Option<String> = None;
                while !stop_for_thread.load(Ordering::Relaxed) {
                    let snapshot = with_android_context(jni_api::current_tag_snapshot)
                        .unwrap_or_else(|error| {
                            panic!("waterkit-nfc: failed to fetch NFC tag snapshot: {error}")
                        });
                    if let Some(snapshot) = snapshot
                        && last_tag_id.as_deref() != Some(snapshot.tag_id.as_str())
                    {
                        let ndef_message = snapshot
                            .ndef_records
                            .as_deref()
                            .map(parse_ndef_records)
                            .transpose()
                            .unwrap_or_else(|error| {
                                panic!("waterkit-nfc: failed to parse NDEF payload: {error}")
                            });
                        let id = hex_decode(&snapshot.tag_id).unwrap_or_else(|error| {
                            panic!("waterkit-nfc: failed to decode NFC tag id: {error}")
                        });

                        {
                            let mut guard = latest_tag_for_thread.lock().unwrap_or_else(|error| {
                                panic!(
                                    "waterkit-nfc: latest tag mutex poisoned in reader thread: {error}"
                                )
                            });
                            *guard = Some(Arc::new(snapshot.tag));
                        }

                        if tag_tx
                            .try_send(NfcTag {
                                id,
                                tag_type: snapshot.tag_type,
                                ndef_message,
                            })
                            .is_err()
                        {
                            break;
                        }

                        last_tag_id = Some(snapshot.tag_id);
                    }

                    std::thread::sleep(Duration::from_millis(250));
                }
            })
            .map_err(|error| NfcError::Platform(format!("spawn NFC session worker: {error}")))?;

        Ok((
            Self {
                latest_tag,
                stop_flag,
                worker: Mutex::new(Some(worker)),
            },
            tag_rx,
        ))
    }

    pub async fn write(&self, message: NdefMessage) -> Result<(), NfcError> {
        let records_json = encode_ndef_records(&message);
        let tag = {
            let guard = self.latest_tag.lock().map_err(|error| {
                NfcError::Platform(format!("latest tag mutex poisoned in write(): {error}"))
            })?;
            guard.clone().ok_or_else(|| {
                NfcError::WriteFailed("no NFC tag discovered in active session".into())
            })?
        };

        std::future::ready(with_android_context(|env, context| {
            jni_api::write_tag(env, context, tag.as_obj(), &records_json)
        }))
        .await
    }

    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Ok(mut worker) = self.worker.lock()
            && let Some(handle) = worker.take()
        {
            let _ = handle.join();
        }

        if let Ok(mut guard) = self.latest_tag.lock() {
            *guard = None;
        }
    }
}

impl Drop for NfcReaderInner {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Ok(worker) = self.worker.get_mut()
            && let Some(handle) = worker.take()
        {
            let _ = handle.join();
        }
    }
}

/// Android-specific NFC functions requiring JNI context.
pub mod jni_api {
    use super::{
        EXTRA_TAG_KEY, Env, JObject, JValue, NfcError, TagSnapshot, decode_string, decode_tag_type,
        is_nfc_intent_action,
    };
    use jni::{jni_sig, jni_str};

    /// Check if NFC is available with JNI context.
    ///
    /// # Errors
    ///
    /// Returns error if JNI operations fail.
    pub fn is_available(env: &mut Env<'_>, context: &JObject<'_>) -> Result<bool, NfcError> {
        let cls = super::HELPER.class(env, context)?;
        env.call_static_method(
            cls,
            jni_str!("isAvailable"),
            jni_sig!("(Landroid/content/Context;)Z"),
            &[JValue::Object(context)],
        )
        .map_err(|e| NfcError::Platform(format!("isAvailable: {e}")))?
        .z()
        .map_err(|e| NfcError::Platform(format!("isAvailable return: {e}")))
    }

    /// Read the currently dispatched NFC tag from Activity intent.
    ///
    /// # Errors
    ///
    /// Returns error if JNI operations fail.
    pub(super) fn current_tag_snapshot(
        env: &mut Env<'_>,
        context: &JObject<'_>,
    ) -> Result<Option<TagSnapshot>, NfcError> {
        let cls = super::HELPER.class(env, context)?;

        let intent = env
            .call_method(
                context,
                jni_str!("getIntent"),
                jni_sig!("()Landroid/content/Intent;"),
                &[],
            )
            .and_then(jni::objects::JValueOwned::l)
            .map_err(|e| NfcError::Platform(format!("Context.getIntent failed: {e}")))?;
        if intent.is_null() {
            return Ok(None);
        }

        let action = env
            .call_method(
                &intent,
                jni_str!("getAction"),
                jni_sig!("()Ljava/lang/String;"),
                &[],
            )
            .and_then(jni::objects::JValueOwned::l)
            .map_err(|e| NfcError::Platform(format!("Intent.getAction failed: {e}")))?;
        if action.is_null() {
            return Ok(None);
        }
        let action_value = decode_string(env, &action)?;
        if !is_nfc_intent_action(&action_value) {
            return Ok(None);
        }

        let tag_extra_key = env
            .new_string(EXTRA_TAG_KEY)
            .map_err(|e| NfcError::Platform(format!("new_string EXTRA_TAG_KEY failed: {e}")))?;
        let tag = env
            .call_method(
                &intent,
                jni_str!("getParcelableExtra"),
                jni_sig!("(Ljava/lang/String;)Landroid/os/Parcelable;"),
                &[JValue::Object(&tag_extra_key)],
            )
            .and_then(jni::objects::JValueOwned::l)
            .map_err(|e| NfcError::Platform(format!("Intent.getParcelableExtra failed: {e}")))?;
        if tag.is_null() {
            return Ok(None);
        }
        let tag_global = env
            .new_global_ref(&tag)
            .map_err(|e| NfcError::Platform(format!("new_global_ref tag failed: {e}")))?;

        let tag_id = env
            .call_static_method(
                cls,
                jni_str!("getTagId"),
                jni_sig!("(Landroid/nfc/Tag;)Ljava/lang/String;"),
                &[JValue::Object(&tag)],
            )
            .and_then(jni::objects::JValueOwned::l)
            .map_err(|e| NfcError::Platform(format!("NfcHelper.getTagId failed: {e}")))?;
        if tag_id.is_null() {
            return Err(NfcError::Platform(
                "NfcHelper.getTagId returned null".into(),
            ));
        }
        let tag_id_value = decode_string(env, &tag_id)?;

        let tag_type = env
            .call_static_method(
                cls,
                jni_str!("getTagType"),
                jni_sig!("(Landroid/nfc/Tag;)I"),
                &[JValue::Object(&tag)],
            )
            .map_err(|e| NfcError::Platform(format!("NfcHelper.getTagType failed: {e}")))?
            .i()
            .map_err(|e| NfcError::Platform(format!("decode tag type failed: {e}")))?;

        let records = env
            .call_static_method(
                cls,
                jni_str!("readTag"),
                jni_sig!("(Landroid/nfc/Tag;)Ljava/lang/String;"),
                &[JValue::Object(&tag)],
            )
            .and_then(jni::objects::JValueOwned::l)
            .map_err(|e| NfcError::Platform(format!("NfcHelper.readTag failed: {e}")))?;
        let records = if records.is_null() {
            None
        } else {
            Some(decode_string(env, &records)?)
        };

        Ok(Some(TagSnapshot {
            tag: tag_global,
            tag_id: tag_id_value,
            tag_type: decode_tag_type(tag_type),
            ndef_records: records,
        }))
    }

    /// Write an NDEF payload to a discovered NFC tag.
    ///
    /// # Errors
    ///
    /// Returns error if writing fails.
    pub(super) fn write_tag(
        env: &mut Env<'_>,
        context: &JObject<'_>,
        tag: &JObject<'_>,
        records_json: &str,
    ) -> Result<(), NfcError> {
        let cls = super::HELPER.class(env, context)?;

        let records = env
            .new_string(records_json)
            .map_err(|e| NfcError::Platform(format!("new_string records_json failed: {e}")))?;
        let error_value = env
            .call_static_method(
                cls,
                jni_str!("writeTag"),
                jni_sig!("(Landroid/nfc/Tag;Ljava/lang/String;)Ljava/lang/String;"),
                &[JValue::Object(tag), JValue::Object(&records)],
            )
            .and_then(jni::objects::JValueOwned::l)
            .map_err(|e| NfcError::Platform(format!("NfcHelper.writeTag failed: {e}")))?;
        if error_value.is_null() {
            return Ok(());
        }

        let error_message = decode_string(env, &error_value)?;
        if error_message.to_ascii_lowercase().contains("read-only") {
            return Err(NfcError::ReadOnly);
        }

        Err(NfcError::WriteFailed(error_message))
    }
}
