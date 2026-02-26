use crate::{NdefMessage, NdefRecord, NfcError, NfcTag, NfcTagType};
use jni::objects::{GlobalRef, JObject, JString, JValue};
use jni::{JNIEnv, JavaVM};
use std::fmt::Write;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));
static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();

const NFC_HELPER_CLASS_NAME: &str = "waterkit.nfc.NfcHelper";
const NFC_ACTION_TAG_DISCOVERED: &str = "android.nfc.action.TAG_DISCOVERED";
const NFC_ACTION_TECH_DISCOVERED: &str = "android.nfc.action.TECH_DISCOVERED";
const NFC_ACTION_NDEF_DISCOVERED: &str = "android.nfc.action.NDEF_DISCOVERED";
const EXTRA_TAG_KEY: &str = "android.nfc.extra.TAG";

fn with_android_context<T, F>(f: F) -> Result<T, NfcError>
where
    F: for<'local> FnOnce(&mut JNIEnv<'local>, &JObject<'local>) -> Result<T, NfcError>,
{
    let android_context = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(android_context.vm().cast()) }
        .map_err(|e| NfcError::PlatformError(format!("JavaVM::from_raw: {e}")))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| NfcError::PlatformError(format!("attach_current_thread: {e}")))?;

    let context = ManuallyDrop::new(unsafe { JObject::from_raw(android_context.context().cast()) });
    assert!(
        !context.is_null(),
        "waterkit-nfc: ndk_context returned null Android Context"
    );

    f(&mut env, &context)
}

fn init_dex(env: &mut JNIEnv, context: &JObject) -> Result<(), NfcError> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }
    let cache_dir = env
        .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| NfcError::PlatformError(format!("getCacheDir: {e}")))?;
    let cache_path = env
        .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| NfcError::PlatformError(format!("getAbsolutePath: {e}")))?;
    let dex_path = format!(
        "{}/waterkit_nfc.dex",
        env.get_string((&cache_path).into())
            .map_err(|e| NfcError::PlatformError(format!("get_string: {e}")))?
            .to_str()
            .map_err(|e| NfcError::PlatformError(format!("to_str: {e}")))?
    );
    let _ = std::fs::remove_file(&dex_path);
    std::fs::write(&dex_path, DEX_BYTES)
        .map_err(|e| NfcError::PlatformError(format!("write DEX: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dex_path)
            .map_err(|e| NfcError::PlatformError(format!("metadata DEX: {e}")))?
            .permissions();
        perms.set_mode(0o444);
        std::fs::set_permissions(&dex_path, perms)
            .map_err(|e| NfcError::PlatformError(format!("set_permissions DEX: {e}")))?;
    }
    let dex_path_jstring = env
        .new_string(&dex_path)
        .map_err(|e| NfcError::PlatformError(format!("new_string: {e}")))?;
    let parent_loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| NfcError::PlatformError(format!("getClassLoader: {e}")))?;
    let dex_class = env
        .find_class("dalvik/system/DexClassLoader")
        .map_err(|e| NfcError::PlatformError(format!("find_class: {e}")))?;
    let loader = env
        .new_object(
            dex_class,
            "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/ClassLoader;)V",
            &[
                JValue::Object(&dex_path_jstring),
                JValue::Object(&cache_path),
                JValue::Object(&JObject::null()),
                JValue::Object(&parent_loader),
            ],
        )
        .map_err(|e| NfcError::PlatformError(format!("new_object: {e}")))?;
    let global = env
        .new_global_ref(loader)
        .map_err(|e| NfcError::PlatformError(format!("global_ref: {e}")))?;
    if CLASS_LOADER.set(global).is_err() {
        assert!(
            CLASS_LOADER.get().is_some(),
            "waterkit-nfc: class loader initialization race left loader unset"
        );
    }
    Ok(())
}

fn helper_class<'local>(
    env: &mut JNIEnv<'local>,
) -> Result<jni::objects::JClass<'local>, NfcError> {
    let helper_class_name = env
        .new_string(NFC_HELPER_CLASS_NAME)
        .map_err(|e| NfcError::PlatformError(format!("new_string: {e}")))?;
    let loader = CLASS_LOADER
        .get()
        .ok_or_else(|| NfcError::PlatformError("Class loader not initialized".into()))?;
    let cls = env
        .call_method(
            loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&helper_class_name)],
        )
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| NfcError::PlatformError(format!("loadClass: {e}")))?;
    Ok(cls.into())
}

fn is_nfc_intent_action(action: &str) -> bool {
    matches!(
        action,
        NFC_ACTION_TAG_DISCOVERED | NFC_ACTION_TECH_DISCOVERED | NFC_ACTION_NDEF_DISCOVERED
    )
}

fn decode_tag_type(code: i32) -> NfcTagType {
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
        return Err(NfcError::PlatformError(format!(
            "invalid hex length for NFC payload: {}",
            hex.len()
        )));
    }

    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for chunk in hex.as_bytes().chunks_exact(2) {
        let value = std::str::from_utf8(chunk)
            .map_err(|e| NfcError::PlatformError(format!("hex utf8 decode failed: {e}")))?;
        let byte = u8::from_str_radix(value, 16)
            .map_err(|e| NfcError::PlatformError(format!("hex parse failed for '{value}': {e}")))?;
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
            .ok_or_else(|| NfcError::PlatformError("missing NDEF TNF".into()))?
            .parse::<u8>()
            .map_err(|e| NfcError::PlatformError(format!("invalid NDEF TNF: {e}")))?;
        let record_type = parts
            .next()
            .ok_or_else(|| NfcError::PlatformError("missing NDEF record type".into()))
            .and_then(hex_decode)?;
        let payload = parts
            .next()
            .ok_or_else(|| NfcError::PlatformError("missing NDEF payload".into()))
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
    tag: GlobalRef,
    tag_id: String,
    tag_type: NfcTagType,
    ndef_records: Option<String>,
}

pub struct NfcReaderInner {
    latest_tag: Arc<Mutex<Option<GlobalRef>>>,
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
                            *guard = Some(snapshot.tag);
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
            .map_err(|error| NfcError::PlatformError(format!("spawn NFC session worker: {error}")))?;

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
                NfcError::PlatformError(format!("latest tag mutex poisoned in write(): {error}"))
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
    use super::*;

    /// Check if NFC is available with JNI context.
    ///
    /// # Errors
    ///
    /// Returns error if JNI operations fail.
    pub fn is_available(env: &mut JNIEnv, context: &JObject) -> Result<bool, NfcError> {
        init_dex(env, context)?;
        let cls = helper_class(env)?;
        env.call_static_method(
            cls,
            "isAvailable",
            "(Landroid/content/Context;)Z",
            &[JValue::Object(context)],
        )
        .map_err(|e| NfcError::PlatformError(format!("isAvailable: {e}")))?
        .z()
        .map_err(|e| NfcError::PlatformError(format!("isAvailable return: {e}")))
    }

    /// Read the currently dispatched NFC tag from Activity intent.
    ///
    /// # Errors
    ///
    /// Returns error if JNI operations fail.
    pub(super) fn current_tag_snapshot(
        env: &mut JNIEnv,
        context: &JObject,
    ) -> Result<Option<TagSnapshot>, NfcError> {
        init_dex(env, context)?;
        let cls = helper_class(env)?;

        let intent = env
            .call_method(context, "getIntent", "()Landroid/content/Intent;", &[])
            .and_then(jni::objects::JValueGen::l)
            .map_err(|e| NfcError::PlatformError(format!("Context.getIntent failed: {e}")))?;
        if intent.is_null() {
            return Ok(None);
        }

        let action = env
            .call_method(&intent, "getAction", "()Ljava/lang/String;", &[])
            .and_then(jni::objects::JValueGen::l)
            .map_err(|e| NfcError::PlatformError(format!("Intent.getAction failed: {e}")))?;
        if action.is_null() {
            return Ok(None);
        }
        let action_value: String = env
            .get_string(&JString::from(action))
            .map_err(|e| NfcError::PlatformError(format!("decode intent action failed: {e}")))?
            .into();
        if !is_nfc_intent_action(&action_value) {
            return Ok(None);
        }

        let tag_extra_key = env.new_string(EXTRA_TAG_KEY).map_err(|e| {
            NfcError::PlatformError(format!("new_string EXTRA_TAG_KEY failed: {e}"))
        })?;
        let tag = env
            .call_method(
                &intent,
                "getParcelableExtra",
                "(Ljava/lang/String;)Landroid/os/Parcelable;",
                &[JValue::Object(&tag_extra_key)],
            )
            .and_then(jni::objects::JValueGen::l)
            .map_err(|e| {
                NfcError::PlatformError(format!("Intent.getParcelableExtra failed: {e}"))
            })?;
        if tag.is_null() {
            return Ok(None);
        }
        let tag_global = env
            .new_global_ref(&tag)
            .map_err(|e| NfcError::PlatformError(format!("new_global_ref tag failed: {e}")))?;

        let tag_id = env
            .call_static_method(
                &cls,
                "getTagId",
                "(Landroid/nfc/Tag;)Ljava/lang/String;",
                &[JValue::Object(&tag)],
            )
            .and_then(jni::objects::JValueGen::l)
            .map_err(|e| NfcError::PlatformError(format!("NfcHelper.getTagId failed: {e}")))?;
        if tag_id.is_null() {
            return Err(NfcError::PlatformError(
                "NfcHelper.getTagId returned null".into(),
            ));
        }
        let tag_id_value: String = env
            .get_string(&JString::from(tag_id))
            .map_err(|e| NfcError::PlatformError(format!("decode tag id failed: {e}")))?
            .into();

        let tag_type = env
            .call_static_method(
                &cls,
                "getTagType",
                "(Landroid/nfc/Tag;)I",
                &[JValue::Object(&tag)],
            )
            .map_err(|e| NfcError::PlatformError(format!("NfcHelper.getTagType failed: {e}")))?
            .i()
            .map_err(|e| NfcError::PlatformError(format!("decode tag type failed: {e}")))?;

        let records = env
            .call_static_method(
                &cls,
                "readTag",
                "(Landroid/nfc/Tag;)Ljava/lang/String;",
                &[JValue::Object(&tag)],
            )
            .and_then(jni::objects::JValueGen::l)
            .map_err(|e| NfcError::PlatformError(format!("NfcHelper.readTag failed: {e}")))?;
        let records = if records.is_null() {
            None
        } else {
            Some(
                env.get_string(&JString::from(records))
                    .map_err(|e| {
                        NfcError::PlatformError(format!("decode NDEF records failed: {e}"))
                    })?
                    .into(),
            )
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
        env: &mut JNIEnv,
        context: &JObject,
        tag: &JObject,
        records_json: &str,
    ) -> Result<(), NfcError> {
        init_dex(env, context)?;
        let cls = helper_class(env)?;

        let records = env
            .new_string(records_json)
            .map_err(|e| NfcError::PlatformError(format!("new_string records_json failed: {e}")))?;
        let error_value = env
            .call_static_method(
                cls,
                "writeTag",
                "(Landroid/nfc/Tag;Ljava/lang/String;)Ljava/lang/String;",
                &[JValue::Object(tag), JValue::Object(&records)],
            )
            .and_then(jni::objects::JValueGen::l)
            .map_err(|e| NfcError::PlatformError(format!("NfcHelper.writeTag failed: {e}")))?;
        if error_value.is_null() {
            return Ok(());
        }

        let error_message: String = env
            .get_string(&JString::from(error_value))
            .map_err(|e| NfcError::PlatformError(format!("decode writeTag error failed: {e}")))?
            .into();
        if error_message.to_ascii_lowercase().contains("read-only") {
            return Err(NfcError::ReadOnly);
        }

        Err(NfcError::WriteFailed(error_message))
    }
}
