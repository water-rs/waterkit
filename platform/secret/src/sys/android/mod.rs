//! Android secret storage implementation using Android Keystore.

use crate::SecretError;
use jni::JNIEnv;
use jni::objects::{JByteArray, JObject, JObjectArray, JString, JValue};
use std::fmt::Display;
use std::mem::ManuallyDrop;

const SHARED_PREFERENCES_NAME: &str = "waterkit_secrets";
const KEY_ALIAS_PREFIX: &str = "waterkit.secret";
const KEYSTORE_PROVIDER: &str = "AndroidKeyStore";
const KEY_ALGORITHM_AES: &str = "AES";
const CIPHER_TRANSFORMATION_AES_GCM: &str = "AES/GCM/NoPadding";
const BLOCK_MODE_GCM: &str = "GCM";
const ENCRYPTION_PADDING_NONE: &str = "NoPadding";
const SHARED_PREFERENCES_MODE_PRIVATE: i32 = 0;
const KEY_PURPOSE_ENCRYPT: i32 = 1;
const KEY_PURPOSE_DECRYPT: i32 = 2;
const CIPHER_MODE_ENCRYPT: i32 = 1;
const CIPHER_MODE_DECRYPT: i32 = 2;
const AES_KEY_SIZE_BITS: i32 = 256;
const GCM_TAG_BITS: i32 = 128;
const BASE64_NO_WRAP: i32 = 2;
const PAYLOAD_SEPARATOR: char = ':';

fn system_error(action: &str, err: impl Display) -> SecretError {
    SecretError::Platform(format!("{action}: {err}"))
}

fn with_android_context<T, F>(f: F) -> Result<T, SecretError>
where
    F: for<'local> FnOnce(&mut JNIEnv<'local>, &JObject<'local>) -> Result<T, SecretError>,
{
    let android_context = ndk_context::android_context();
    let vm = unsafe { jni::JavaVM::from_raw(android_context.vm().cast()) }
        .map_err(|err| system_error("failed to obtain JavaVM from ndk_context", err))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|err| system_error("failed to attach current thread to JVM", err))?;
    let context = ManuallyDrop::new(unsafe { JObject::from_raw(android_context.context().cast()) });
    if context.is_null() {
        return Err(SecretError::Platform(
            "failed to obtain Android Context from ndk_context".into(),
        ));
    }
    f(&mut env, &context)
}

fn entry_identifier(service: &str, account: &str) -> String {
    format!("{}:{service}:{}:{account}", service.len(), account.len())
}

fn preference_key(service: &str, account: &str) -> String {
    entry_identifier(service, account)
}

fn key_alias(service: &str, account: &str) -> String {
    format!("{KEY_ALIAS_PREFIX}.{}", entry_identifier(service, account))
}

fn get_shared_preferences<'local>(
    env: &mut JNIEnv<'local>,
    context: &JObject<'local>,
) -> Result<JObject<'local>, SecretError> {
    let preferences_name = env
        .new_string(SHARED_PREFERENCES_NAME)
        .map_err(|err| system_error("failed to allocate SharedPreferences name", err))?;

    env.call_method(
        context,
        "getSharedPreferences",
        "(Ljava/lang/String;I)Landroid/content/SharedPreferences;",
        &[
            JValue::Object(&preferences_name),
            JValue::Int(SHARED_PREFERENCES_MODE_PRIVATE),
        ],
    )
    .map_err(|err| system_error("SharedPreferences lookup failed", err))?
    .l()
    .map_err(|err| system_error("SharedPreferences JNI result conversion failed", err))
}

fn get_preferences_editor<'local>(
    env: &mut JNIEnv<'local>,
    preferences: &JObject<'local>,
) -> Result<JObject<'local>, SecretError> {
    env.call_method(
        preferences,
        "edit",
        "()Landroid/content/SharedPreferences$Editor;",
        &[],
    )
    .map_err(|err| system_error("SharedPreferences.edit() failed", err))?
    .l()
    .map_err(|err| system_error("SharedPreferences editor JNI result conversion failed", err))
}

fn apply_editor<'local>(
    env: &mut JNIEnv<'local>,
    editor: &JObject<'local>,
) -> Result<(), SecretError> {
    env.call_method(editor, "apply", "()V", &[])
        .map_err(|err| system_error("SharedPreferences.Editor.apply() failed", err))?;
    Ok(())
}

fn load_keystore<'local>(env: &mut JNIEnv<'local>) -> Result<JObject<'local>, SecretError> {
    let provider = env
        .new_string(KEYSTORE_PROVIDER)
        .map_err(|err| system_error("failed to allocate keystore provider string", err))?;

    let keystore = env
        .call_static_method(
            "java/security/KeyStore",
            "getInstance",
            "(Ljava/lang/String;)Ljava/security/KeyStore;",
            &[JValue::Object(&provider)],
        )
        .map_err(|err| system_error("KeyStore.getInstance failed", err))?
        .l()
        .map_err(|err| system_error("KeyStore JNI result conversion failed", err))?;

    let null_input_stream = JObject::null();
    let null_password = JObject::null();
    env.call_method(
        &keystore,
        "load",
        "(Ljava/io/InputStream;[C)V",
        &[
            JValue::Object(&null_input_stream),
            JValue::Object(&null_password),
        ],
    )
    .map_err(|err| system_error("KeyStore.load failed", err))?;

    Ok(keystore)
}

fn keystore_contains_alias<'local>(
    env: &mut JNIEnv<'local>,
    keystore: &JObject<'local>,
    alias: &str,
) -> Result<bool, SecretError> {
    let alias_jstring = env
        .new_string(alias)
        .map_err(|err| system_error("failed to allocate keystore alias", err))?;

    env.call_method(
        keystore,
        "containsAlias",
        "(Ljava/lang/String;)Z",
        &[JValue::Object(&alias_jstring)],
    )
    .map_err(|err| system_error("KeyStore.containsAlias failed", err))?
    .z()
    .map_err(|err| system_error("KeyStore.containsAlias JNI result conversion failed", err))
}

fn single_string_array<'local>(
    env: &mut JNIEnv<'local>,
    value: &str,
) -> Result<JObjectArray<'local>, SecretError> {
    let array = env
        .new_object_array(1, "java/lang/String", JObject::null())
        .map_err(|err| system_error("failed to allocate String[]", err))?;
    let value_jstring = env
        .new_string(value)
        .map_err(|err| system_error("failed to allocate String value", err))?;
    env.set_object_array_element(&array, 0, &value_jstring)
        .map_err(|err| system_error("failed to populate String[]", err))?;
    Ok(array)
}

fn generate_secret_key(env: &mut JNIEnv<'_>, alias: &str) -> Result<(), SecretError> {
    let alias_jstring = env
        .new_string(alias)
        .map_err(|err| system_error("failed to allocate key alias", err))?;

    let purposes = KEY_PURPOSE_ENCRYPT | KEY_PURPOSE_DECRYPT;
    let builder = env
        .new_object(
            "android/security/keystore/KeyGenParameterSpec$Builder",
            "(Ljava/lang/String;I)V",
            &[JValue::Object(&alias_jstring), JValue::Int(purposes)],
        )
        .map_err(|err| system_error("KeyGenParameterSpec.Builder init failed", err))?;

    let block_modes = single_string_array(env, BLOCK_MODE_GCM)?;
    env.call_method(
        &builder,
        "setBlockModes",
        "([Ljava/lang/String;)Landroid/security/keystore/KeyGenParameterSpec$Builder;",
        &[JValue::Object(block_modes.as_ref())],
    )
    .map_err(|err| system_error("KeyGenParameterSpec.setBlockModes failed", err))?;

    let paddings = single_string_array(env, ENCRYPTION_PADDING_NONE)?;
    env.call_method(
        &builder,
        "setEncryptionPaddings",
        "([Ljava/lang/String;)Landroid/security/keystore/KeyGenParameterSpec$Builder;",
        &[JValue::Object(paddings.as_ref())],
    )
    .map_err(|err| system_error("KeyGenParameterSpec.setEncryptionPaddings failed", err))?;

    env.call_method(
        &builder,
        "setKeySize",
        "(I)Landroid/security/keystore/KeyGenParameterSpec$Builder;",
        &[JValue::Int(AES_KEY_SIZE_BITS)],
    )
    .map_err(|err| system_error("KeyGenParameterSpec.setKeySize failed", err))?;

    let spec = env
        .call_method(
            &builder,
            "build",
            "()Landroid/security/keystore/KeyGenParameterSpec;",
            &[],
        )
        .map_err(|err| system_error("KeyGenParameterSpec.build failed", err))?
        .l()
        .map_err(|err| system_error("KeyGenParameterSpec JNI result conversion failed", err))?;

    let algorithm = env
        .new_string(KEY_ALGORITHM_AES)
        .map_err(|err| system_error("failed to allocate key algorithm string", err))?;
    let provider = env
        .new_string(KEYSTORE_PROVIDER)
        .map_err(|err| system_error("failed to allocate key provider string", err))?;
    let key_generator = env
        .call_static_method(
            "javax/crypto/KeyGenerator",
            "getInstance",
            "(Ljava/lang/String;Ljava/lang/String;)Ljavax/crypto/KeyGenerator;",
            &[JValue::Object(&algorithm), JValue::Object(&provider)],
        )
        .map_err(|err| system_error("KeyGenerator.getInstance failed", err))?
        .l()
        .map_err(|err| system_error("KeyGenerator JNI result conversion failed", err))?;

    env.call_method(
        &key_generator,
        "init",
        "(Ljava/security/spec/AlgorithmParameterSpec;)V",
        &[JValue::Object(&spec)],
    )
    .map_err(|err| system_error("KeyGenerator.init failed", err))?;

    env.call_method(
        &key_generator,
        "generateKey",
        "()Ljavax/crypto/SecretKey;",
        &[],
    )
    .map_err(|err| system_error("KeyGenerator.generateKey failed", err))?;

    Ok(())
}

fn ensure_hardware_backed<'local>(
    env: &mut JNIEnv<'local>,
    key: &JObject<'local>,
) -> Result<(), SecretError> {
    let algorithm = env
        .call_method(key, "getAlgorithm", "()Ljava/lang/String;", &[])
        .map_err(|err| system_error("SecretKey.getAlgorithm failed", err))?
        .l()
        .map_err(|err| system_error("SecretKey algorithm JNI result conversion failed", err))?;
    let provider = env
        .new_string(KEYSTORE_PROVIDER)
        .map_err(|err| system_error("failed to allocate provider string", err))?;

    let secret_key_factory = env
        .call_static_method(
            "javax/crypto/SecretKeyFactory",
            "getInstance",
            "(Ljava/lang/String;Ljava/lang/String;)Ljavax/crypto/SecretKeyFactory;",
            &[
                JValue::Object(algorithm.as_ref()),
                JValue::Object(&provider),
            ],
        )
        .map_err(|err| system_error("SecretKeyFactory.getInstance failed", err))?
        .l()
        .map_err(|err| system_error("SecretKeyFactory JNI result conversion failed", err))?;

    let key_info_class = env
        .find_class("android/security/keystore/KeyInfo")
        .map_err(|err| {
            system_error("android.security.keystore.KeyInfo class lookup failed", err)
        })?;
    let key_info = env
        .call_method(
            &secret_key_factory,
            "getKeySpec",
            "(Ljava/security/Key;Ljava/lang/Class;)Ljava/security/spec/KeySpec;",
            &[JValue::Object(key), JValue::Object(key_info_class.as_ref())],
        )
        .map_err(|err| system_error("SecretKeyFactory.getKeySpec failed", err))?
        .l()
        .map_err(|err| {
            system_error(
                "SecretKeyFactory.getKeySpec JNI result conversion failed",
                err,
            )
        })?;

    let inside_secure_hardware = env
        .call_method(&key_info, "isInsideSecureHardware", "()Z", &[])
        .map_err(|err| system_error("KeyInfo.isInsideSecureHardware failed", err))?
        .z()
        .map_err(|err| {
            system_error(
                "KeyInfo.isInsideSecureHardware JNI result conversion failed",
                err,
            )
        })?;

    if inside_secure_hardware {
        Ok(())
    } else {
        Err(SecretError::Platform(
            "Android keystore key is not hardware-backed".into(),
        ))
    }
}

fn ensure_secret_key<'local>(
    env: &mut JNIEnv<'local>,
    alias: &str,
) -> Result<JObject<'local>, SecretError> {
    let keystore = load_keystore(env)?;
    if !keystore_contains_alias(env, &keystore, alias)? {
        generate_secret_key(env, alias)?;
    }

    let keystore = load_keystore(env)?;
    let alias_jstring = env
        .new_string(alias)
        .map_err(|err| system_error("failed to allocate key alias", err))?;
    let null_password = JObject::null();
    let key = env
        .call_method(
            &keystore,
            "getKey",
            "(Ljava/lang/String;[C)Ljava/security/Key;",
            &[
                JValue::Object(&alias_jstring),
                JValue::Object(&null_password),
            ],
        )
        .map_err(|err| system_error("KeyStore.getKey failed", err))?
        .l()
        .map_err(|err| system_error("KeyStore.getKey JNI result conversion failed", err))?;

    if key.is_null() {
        return Err(SecretError::Platform(
            "Android keystore returned null key".into(),
        ));
    }

    ensure_hardware_backed(env, &key)?;
    Ok(key)
}

fn get_cipher<'local>(env: &mut JNIEnv<'local>) -> Result<JObject<'local>, SecretError> {
    let transformation = env
        .new_string(CIPHER_TRANSFORMATION_AES_GCM)
        .map_err(|err| system_error("failed to allocate cipher transformation string", err))?;
    env.call_static_method(
        "javax/crypto/Cipher",
        "getInstance",
        "(Ljava/lang/String;)Ljavax/crypto/Cipher;",
        &[JValue::Object(&transformation)],
    )
    .map_err(|err| system_error("Cipher.getInstance failed", err))?
    .l()
    .map_err(|err| system_error("Cipher JNI result conversion failed", err))
}

fn encode_base64<'local>(
    env: &mut JNIEnv<'local>,
    data: &JByteArray<'local>,
) -> Result<String, SecretError> {
    let encoded = env
        .call_static_method(
            "android/util/Base64",
            "encodeToString",
            "([BI)Ljava/lang/String;",
            &[JValue::Object(data.as_ref()), JValue::Int(BASE64_NO_WRAP)],
        )
        .map_err(|err| system_error("Base64.encodeToString failed", err))?
        .l()
        .map_err(|err| system_error("Base64.encodeToString JNI result conversion failed", err))?;

    let encoded_jstring: JString = encoded.into();
    env.get_string(&encoded_jstring)
        .map_err(|err| system_error("failed to read Base64 encoded string", err))
        .map(Into::into)
}

fn decode_base64<'local>(
    env: &mut JNIEnv<'local>,
    encoded: &str,
) -> Result<JByteArray<'local>, SecretError> {
    let encoded_jstring = env
        .new_string(encoded)
        .map_err(|err| system_error("failed to allocate Base64 input string", err))?;
    let decoded = env
        .call_static_method(
            "android/util/Base64",
            "decode",
            "(Ljava/lang/String;I)[B",
            &[
                JValue::Object(&encoded_jstring),
                JValue::Int(BASE64_NO_WRAP),
            ],
        )
        .map_err(|err| system_error("Base64.decode failed", err))?
        .l()
        .map_err(|err| system_error("Base64.decode JNI result conversion failed", err))?;
    Ok(decoded.into())
}

fn encrypt_payload<'local>(
    env: &mut JNIEnv<'local>,
    key: &JObject<'local>,
    plaintext: &str,
) -> Result<String, SecretError> {
    let cipher = get_cipher(env)?;
    env.call_method(
        &cipher,
        "init",
        "(ILjava/security/Key;)V",
        &[JValue::Int(CIPHER_MODE_ENCRYPT), JValue::Object(key)],
    )
    .map_err(|err| system_error("Cipher.init (encrypt) failed", err))?;

    let plaintext_bytes = env
        .byte_array_from_slice(plaintext.as_bytes())
        .map_err(|err| system_error("failed to allocate plaintext byte array", err))?;
    let ciphertext: JByteArray = env
        .call_method(
            &cipher,
            "doFinal",
            "([B)[B",
            &[JValue::Object(plaintext_bytes.as_ref())],
        )
        .map_err(|err| system_error("Cipher.doFinal (encrypt) failed", err))?
        .l()
        .map_err(|err| system_error("Cipher.doFinal JNI result conversion failed", err))?
        .into();
    let iv: JByteArray = env
        .call_method(&cipher, "getIV", "()[B", &[])
        .map_err(|err| system_error("Cipher.getIV failed", err))?
        .l()
        .map_err(|err| system_error("Cipher.getIV JNI result conversion failed", err))?
        .into();

    let iv_encoded = encode_base64(env, &iv)?;
    let ciphertext_encoded = encode_base64(env, &ciphertext)?;
    Ok(format!(
        "{iv_encoded}{PAYLOAD_SEPARATOR}{ciphertext_encoded}"
    ))
}

fn decrypt_payload<'local>(
    env: &mut JNIEnv<'local>,
    key: &JObject<'local>,
    payload: &str,
) -> Result<String, SecretError> {
    let (iv_encoded, ciphertext_encoded) =
        payload.split_once(PAYLOAD_SEPARATOR).ok_or_else(|| {
            SecretError::Platform(
                "stored Android secret payload is malformed (missing separator)".into(),
            )
        })?;

    let iv = decode_base64(env, iv_encoded)?;
    let ciphertext = decode_base64(env, ciphertext_encoded)?;

    let gcm_spec = env
        .new_object(
            "javax/crypto/spec/GCMParameterSpec",
            "(I[B)V",
            &[JValue::Int(GCM_TAG_BITS), JValue::Object(iv.as_ref())],
        )
        .map_err(|err| system_error("GCMParameterSpec creation failed", err))?;

    let cipher = get_cipher(env)?;
    env.call_method(
        &cipher,
        "init",
        "(ILjava/security/Key;Ljava/security/spec/AlgorithmParameterSpec;)V",
        &[
            JValue::Int(CIPHER_MODE_DECRYPT),
            JValue::Object(key),
            JValue::Object(&gcm_spec),
        ],
    )
    .map_err(|err| system_error("Cipher.init (decrypt) failed", err))?;

    let plaintext: JByteArray = env
        .call_method(
            &cipher,
            "doFinal",
            "([B)[B",
            &[JValue::Object(ciphertext.as_ref())],
        )
        .map_err(|err| system_error("Cipher.doFinal (decrypt) failed", err))?
        .l()
        .map_err(|err| system_error("Cipher.doFinal JNI result conversion failed", err))?
        .into();
    let plaintext_bytes = env
        .convert_byte_array(&plaintext)
        .map_err(|err| system_error("failed to convert plaintext byte array", err))?;

    String::from_utf8(plaintext_bytes).map_err(|err| {
        SecretError::Platform(format!("decrypted payload is not valid UTF-8: {err}"))
    })
}

#[allow(clippy::unused_async)]
pub async fn set(service: &str, account: &str, password: &str) -> Result<(), SecretError> {
    with_android_context(|env, context| set_with_context(env, context, service, account, password))
}

#[allow(clippy::unused_async)]
pub async fn get(service: &str, account: &str) -> Result<String, SecretError> {
    with_android_context(|env, context| get_with_context(env, context, service, account))
}

#[allow(clippy::unused_async)]
pub async fn delete(service: &str, account: &str) -> Result<(), SecretError> {
    with_android_context(|env, context| delete_with_context(env, context, service, account))
}

/// Save a secret using Android `KeyStore` with AES-GCM encryption.
///
/// # Errors
///
/// Returns [`SecretError`] when key generation, encryption, or `SharedPreferences` writes fail.
pub fn set_with_context<'local>(
    env: &mut JNIEnv<'local>,
    context: &JObject<'local>,
    service: &str,
    account: &str,
    password: &str,
) -> Result<(), SecretError> {
    let entry_key = preference_key(service, account);
    let alias = key_alias(service, account);
    let key = ensure_secret_key(env, &alias)?;
    let payload = encrypt_payload(env, &key, password)?;

    let preferences = get_shared_preferences(env, context)?;
    let editor = get_preferences_editor(env, &preferences)?;
    let entry_key_jstring = env
        .new_string(entry_key)
        .map_err(|err| system_error("failed to allocate SharedPreferences key", err))?;
    let payload_jstring = env
        .new_string(payload)
        .map_err(|err| system_error("failed to allocate encrypted payload string", err))?;

    env.call_method(
        &editor,
        "putString",
        "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/SharedPreferences$Editor;",
        &[
            JValue::Object(&entry_key_jstring),
            JValue::Object(&payload_jstring),
        ],
    )
    .map_err(|err| system_error("SharedPreferences.Editor.putString failed", err))?;
    apply_editor(env, &editor)
}

/// Retrieve a secret using Android `KeyStore` with AES-GCM decryption.
///
/// # Errors
///
/// Returns [`SecretError`] when key retrieval, `SharedPreferences` reads, or decryption fails.
pub fn get_with_context<'local>(
    env: &mut JNIEnv<'local>,
    context: &JObject<'local>,
    service: &str,
    account: &str,
) -> Result<String, SecretError> {
    let entry_key = preference_key(service, account);
    let alias = key_alias(service, account);

    let preferences = get_shared_preferences(env, context)?;
    let entry_key_jstring = env
        .new_string(entry_key)
        .map_err(|err| system_error("failed to allocate SharedPreferences key", err))?;
    let null_default = JObject::null();
    let stored_payload = env
        .call_method(
            &preferences,
            "getString",
            "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
            &[
                JValue::Object(&entry_key_jstring),
                JValue::Object(&null_default),
            ],
        )
        .map_err(|err| system_error("SharedPreferences.getString failed", err))?
        .l()
        .map_err(|err| {
            system_error(
                "SharedPreferences.getString JNI result conversion failed",
                err,
            )
        })?;

    if stored_payload.is_null() {
        return Err(SecretError::NotFound);
    }

    let stored_payload_java: JString = stored_payload.into();
    let stored_payload_text: String = env
        .get_string(&stored_payload_java)
        .map_err(|err| system_error("failed to read encrypted payload string", err))?
        .into();

    let key = ensure_secret_key(env, &alias)?;
    decrypt_payload(env, &key, &stored_payload_text)
}

/// Delete a secret and remove its associated Android `KeyStore` entry.
///
/// # Errors
///
/// Returns [`SecretError`] when `SharedPreferences` updates or key deletion operations fail.
pub fn delete_with_context<'local>(
    env: &mut JNIEnv<'local>,
    context: &JObject<'local>,
    service: &str,
    account: &str,
) -> Result<(), SecretError> {
    let entry_key = preference_key(service, account);
    let alias = key_alias(service, account);

    let preferences = get_shared_preferences(env, context)?;
    let editor = get_preferences_editor(env, &preferences)?;
    let entry_key_jstring = env
        .new_string(entry_key)
        .map_err(|err| system_error("failed to allocate SharedPreferences key", err))?;
    env.call_method(
        &editor,
        "remove",
        "(Ljava/lang/String;)Landroid/content/SharedPreferences$Editor;",
        &[JValue::Object(&entry_key_jstring)],
    )
    .map_err(|err| system_error("SharedPreferences.Editor.remove failed", err))?;
    apply_editor(env, &editor)?;

    let keystore = load_keystore(env)?;
    if keystore_contains_alias(env, &keystore, &alias)? {
        let alias_jstring = env
            .new_string(alias)
            .map_err(|err| system_error("failed to allocate key alias", err))?;
        env.call_method(
            &keystore,
            "deleteEntry",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&alias_jstring)],
        )
        .map_err(|err| system_error("KeyStore.deleteEntry failed", err))?;
    }

    Ok(())
}
