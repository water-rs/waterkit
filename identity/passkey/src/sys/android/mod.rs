//! Android passkey backend via dynamically loaded Kotlin helper.

use std::mem::ManuallyDrop;

use async_trait::async_trait;
use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jlong};
use jni::{JNIEnv, NativeMethod};

use crate::{
    AuthenticateOptions, AuthenticationResult, Availability, PasskeyError, RegisterOptions,
    RegistrationResult, authenticate_request_json, parse_authentication_response_json,
    parse_registration_response_json, register_request_json,
};

use super::PasskeyBackend;

const DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

type RegisterSender = tokio::sync::oneshot::Sender<Result<RegistrationResult, PasskeyError>>;
type AuthenticateSender = tokio::sync::oneshot::Sender<Result<AuthenticationResult, PasskeyError>>;

pub struct PlatformBackend;

#[async_trait]
impl PasskeyBackend for PlatformBackend {
    async fn is_available(&self) -> Result<Availability, PasskeyError> {
        let available = with_android_context(|env, context| {
            let class_loader = prepare_class_loader(env, context)?;
            let helper_class = get_helper_class(env, &class_loader)?;
            let value = env
                .call_static_method(
                    helper_class,
                    "isAvailable",
                    "(Landroid/content/Context;)Z",
                    &[JValue::Object(context)],
                )
                .map_err(|error| {
                    PasskeyError::Platform(format!("isAvailable call failed: {error}"))
                })?
                .z()
                .map_err(|error| {
                    PasskeyError::Platform(format!("isAvailable return conversion failed: {error}"))
                })?;
            Ok(value)
        })
        .map_err(|error| {
            PasskeyError::Platform(format!("android availability check failed: {error}"))
        })?;

        if available {
            Ok(Availability::supported())
        } else {
            Ok(Availability::unavailable())
        }
    }

    async fn register(
        &self,
        options: &RegisterOptions,
    ) -> Result<RegistrationResult, PasskeyError> {
        let request_json = register_request_json(options)?;
        let rx = with_android_context(|env, context| {
            register_with_context(env, context, &request_json)
        })?;

        rx.await.unwrap_or_else(|_| {
            Err(PasskeyError::Platform(
                "android registration callback channel closed".into(),
            ))
        })
    }

    async fn authenticate(
        &self,
        options: &AuthenticateOptions,
    ) -> Result<AuthenticationResult, PasskeyError> {
        let request_json = authenticate_request_json(options)?;
        let rx = with_android_context(|env, context| {
            authenticate_with_context(env, context, &request_json)
        })?;

        rx.await.unwrap_or_else(|_| {
            Err(PasskeyError::Platform(
                "android authentication callback channel closed".into(),
            ))
        })
    }
}

fn with_android_context<T, F>(f: F) -> Result<T, PasskeyError>
where
    F: for<'local> FnOnce(&mut JNIEnv<'local>, &JObject<'local>) -> Result<T, PasskeyError>,
{
    let android_context = ndk_context::android_context();
    let vm = unsafe { jni::JavaVM::from_raw(android_context.vm().cast()) }
        .map_err(|error| PasskeyError::Platform(format!("from_raw JavaVM failed: {error}")))?;

    let mut env = vm.attach_current_thread().map_err(|error| {
        PasskeyError::Platform(format!("attach_current_thread failed: {error}"))
    })?;

    let context = ManuallyDrop::new(unsafe { JObject::from_raw(android_context.context().cast()) });
    if context.is_null() {
        return Err(PasskeyError::Platform(
            "android context is null from ndk_context".into(),
        ));
    }

    f(&mut env, &context)
}

fn prepare_class_loader(env: &mut JNIEnv, context: &JObject) -> Result<GlobalRef, PasskeyError> {
    let cache_dir = env
        .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])
        .map_err(|error| PasskeyError::Platform(format!("getCacheDir failed: {error}")))?
        .l()
        .map_err(|error| PasskeyError::Platform(format!("getCacheDir result invalid: {error}")))?;

    let cache_path = env
        .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .map_err(|error| PasskeyError::Platform(format!("getAbsolutePath failed: {error}")))?
        .l()
        .map_err(|error| {
            PasskeyError::Platform(format!("getAbsolutePath result invalid: {error}"))
        })?;

    let cache_path_string: String = env
        .get_string(&JString::from(cache_path))
        .map_err(|error| PasskeyError::Platform(format!("cache path decode failed: {error}")))?
        .into();

    let dex_path = format!(
        "{cache_path_string}/waterkit_passkey_{}.dex",
        std::process::id()
    );

    let _ = std::fs::remove_file(&dex_path);
    std::fs::write(&dex_path, DEX_BYTES)
        .map_err(|error| PasskeyError::Platform(format!("write DEX failed: {error}")))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = std::fs::metadata(&dex_path)
            .map_err(|error| PasskeyError::Platform(format!("dex metadata failed: {error}")))?
            .permissions();
        permissions.set_mode(0o444);
        std::fs::set_permissions(&dex_path, permissions).map_err(|error| {
            PasskeyError::Platform(format!("set dex permissions failed: {error}"))
        })?;
    }

    let dex_path_java = env
        .new_string(dex_path)
        .map_err(|error| PasskeyError::Platform(format!("new dex path string failed: {error}")))?;
    let cache_path_java = env.new_string(cache_path_string).map_err(|error| {
        PasskeyError::Platform(format!("new cache path string failed: {error}"))
    })?;

    let parent_loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .map_err(|error| PasskeyError::Platform(format!("getClassLoader failed: {error}")))?
        .l()
        .map_err(|error| {
            PasskeyError::Platform(format!("getClassLoader result invalid: {error}"))
        })?;

    let dex_class_loader = env
        .find_class("dalvik/system/DexClassLoader")
        .map_err(|error| PasskeyError::Platform(format!("find DexClassLoader failed: {error}")))?;

    let loader = env
        .new_object(
            dex_class_loader,
            "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/ClassLoader;)V",
            &[
                JValue::Object(&dex_path_java),
                JValue::Object(&cache_path_java),
                JValue::Object(&JObject::null()),
                JValue::Object(&parent_loader),
            ],
        )
        .map_err(|error| PasskeyError::Platform(format!("new DexClassLoader failed: {error}")))?;

    env.new_global_ref(loader)
        .map_err(|error| PasskeyError::Platform(format!("new_global_ref failed: {error}")))
}

fn get_helper_class<'local>(
    env: &mut JNIEnv<'local>,
    class_loader: &GlobalRef,
) -> Result<JClass<'local>, PasskeyError> {
    let class_name = env
        .new_string("waterkit.passkey.PasskeyHelper")
        .map_err(|error| {
            PasskeyError::Platform(format!("new helper class string failed: {error}"))
        })?;

    let loaded_class = env
        .call_method(
            class_loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&class_name)],
        )
        .map_err(|error| PasskeyError::Platform(format!("loadClass failed: {error}")))?
        .l()
        .map_err(|error| PasskeyError::Platform(format!("loadClass result invalid: {error}")))?;

    Ok(JClass::from(loaded_class))
}

fn register_natives(env: &mut JNIEnv, helper_class: &JClass) -> Result<(), PasskeyError> {
    let _ = env.unregister_native_methods(helper_class);

    let methods = [
        NativeMethod {
            name: "onRegisterResult".into(),
            sig: "(JZLjava/lang/String;Ljava/lang/String;)V".into(),
            fn_ptr: Java_waterkit_passkey_PasskeyHelper_onRegisterResult as *mut _,
        },
        NativeMethod {
            name: "onAuthenticateResult".into(),
            sig: "(JZLjava/lang/String;Ljava/lang/String;)V".into(),
            fn_ptr: Java_waterkit_passkey_PasskeyHelper_onAuthenticateResult as *mut _,
        },
    ];

    env.register_native_methods(helper_class, &methods)
        .map_err(|error| PasskeyError::Platform(format!("register_native_methods failed: {error}")))
}

fn register_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    request_json: &str,
) -> Result<tokio::sync::oneshot::Receiver<Result<RegistrationResult, PasskeyError>>, PasskeyError>
{
    let class_loader = prepare_class_loader(env, context)?;
    let helper_class = get_helper_class(env, &class_loader)?;
    register_natives(env, &helper_class)?;

    let request_json_java = env.new_string(request_json).map_err(|error| {
        PasskeyError::Platform(format!("new registration JSON string failed: {error}"))
    })?;

    let (tx, rx) = tokio::sync::oneshot::channel();
    let sender_ptr = Box::into_raw(Box::new(tx)) as jlong;

    if let Err(error) = env.call_static_method(
        &helper_class,
        "register",
        "(Landroid/content/Context;Ljava/lang/String;J)V",
        &[
            JValue::Object(context),
            JValue::Object(&request_json_java),
            JValue::Long(sender_ptr),
        ],
    ) {
        let _ = unsafe { Box::from_raw(sender_ptr as *mut RegisterSender) };
        return Err(PasskeyError::Platform(format!(
            "PasskeyHelper.register invocation failed: {error}"
        )));
    }

    Ok(rx)
}

fn authenticate_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    request_json: &str,
) -> Result<tokio::sync::oneshot::Receiver<Result<AuthenticationResult, PasskeyError>>, PasskeyError>
{
    let class_loader = prepare_class_loader(env, context)?;
    let helper_class = get_helper_class(env, &class_loader)?;
    register_natives(env, &helper_class)?;

    let request_json_java = env
        .new_string(request_json)
        .map_err(|error| PasskeyError::Platform(format!("new auth JSON string failed: {error}")))?;

    let (tx, rx) = tokio::sync::oneshot::channel();
    let sender_ptr = Box::into_raw(Box::new(tx)) as jlong;

    if let Err(error) = env.call_static_method(
        &helper_class,
        "authenticate",
        "(Landroid/content/Context;Ljava/lang/String;J)V",
        &[
            JValue::Object(context),
            JValue::Object(&request_json_java),
            JValue::Long(sender_ptr),
        ],
    ) {
        let _ = unsafe { Box::from_raw(sender_ptr as *mut AuthenticateSender) };
        return Err(PasskeyError::Platform(format!(
            "PasskeyHelper.authenticate invocation failed: {error}"
        )));
    }

    Ok(rx)
}

fn jobject_to_option_string(env: &mut JNIEnv, object: JObject) -> Option<String> {
    if object.is_null() {
        return None;
    }

    env.get_string(&JString::from(object)).ok().map(Into::into)
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn Java_waterkit_passkey_PasskeyHelper_onRegisterResult(
    mut env: JNIEnv,
    _class: JClass,
    callback_ptr: jlong,
    success: jboolean,
    error_message: JObject,
    response_json: JObject,
) {
    if callback_ptr == 0 {
        return;
    }

    let sender = unsafe { Box::from_raw(callback_ptr as *mut RegisterSender) };

    if success != 0 {
        let payload = jobject_to_option_string(&mut env, response_json).ok_or_else(|| {
            PasskeyError::Platform("android registration callback missing response payload".into())
        });

        let result = payload.and_then(|json| parse_registration_response_json(&json));
        let _ = sender.send(result);
        return;
    }

    let error = jobject_to_option_string(&mut env, error_message).map_or_else(
        || PasskeyError::Platform("android registration failed without error message".into()),
        PasskeyError::from_platform_error,
    );
    let _ = sender.send(Err(error));
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn Java_waterkit_passkey_PasskeyHelper_onAuthenticateResult(
    mut env: JNIEnv,
    _class: JClass,
    callback_ptr: jlong,
    success: jboolean,
    error_message: JObject,
    response_json: JObject,
) {
    if callback_ptr == 0 {
        return;
    }

    let sender = unsafe { Box::from_raw(callback_ptr as *mut AuthenticateSender) };

    if success != 0 {
        let payload = jobject_to_option_string(&mut env, response_json).ok_or_else(|| {
            PasskeyError::Platform(
                "android authentication callback missing response payload".into(),
            )
        });

        let result = payload.and_then(|json| parse_authentication_response_json(&json));
        let _ = sender.send(result);
        return;
    }

    let error = jobject_to_option_string(&mut env, error_message).map_or_else(
        || PasskeyError::Platform("android authentication failed without error message".into()),
        PasskeyError::from_platform_error,
    );
    let _ = sender.send(Err(error));
}
