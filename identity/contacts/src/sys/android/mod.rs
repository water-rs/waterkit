use crate::{Contact, ContactData, ContactsError, EmailAddress, PhoneNumber};
use futures::future;
use jni::objects::{GlobalRef, JClass, JObject, JObjectArray, JString, JValue};
use jni::{JNIEnv, JavaVM};
use std::fmt::Display;
use std::mem::ManuallyDrop;
use std::sync::OnceLock;

static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));
static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();

fn with_android_context<T, F>(f: F) -> Result<T, ContactsError>
where
    F: for<'local> FnOnce(&mut JNIEnv<'local>, &JObject<'local>) -> Result<T, ContactsError>,
{
    let android_context = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(android_context.vm().cast()) }
        .map_err(|e| platform_error("JavaVM::from_raw", e))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| platform_error("attach_current_thread", e))?;
    let context = ManuallyDrop::new(unsafe { JObject::from_raw(android_context.context().cast()) });
    assert!(
        !context.is_null(),
        "waterkit-contacts: ndk_context returned null Android Context"
    );
    f(&mut env, &context)
}

fn init_with_context(env: &mut JNIEnv, context: &JObject) -> Result<(), ContactsError> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }

    let cache_dir = env
        .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| map_jni_error(env, "getCacheDir", e))?;

    let cache_path = env
        .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| map_jni_error(env, "getAbsolutePath", e))?;

    let cache_path_str = env
        .get_string((&cache_path).into())
        .map_err(|e| map_jni_error(env, "cache path get_string", e))?
        .to_str()
        .map_err(|e| platform_error("cache path to_str", e))?
        .to_owned();
    let dex_path = format!("{cache_path_str}/waterkit_contacts.dex");

    let _ = std::fs::remove_file(&dex_path);
    std::fs::write(&dex_path, DEX_BYTES).map_err(|e| platform_error("write DEX", e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dex_path)
            .map_err(|e| platform_error("metadata DEX", e))?
            .permissions();
        perms.set_mode(0o444);
        std::fs::set_permissions(&dex_path, perms)
            .map_err(|e| platform_error("set_permissions DEX", e))?;
    }

    let dex_path_jstring = env
        .new_string(&dex_path)
        .map_err(|e| map_jni_error(env, "new_string dex_path", e))?;
    let parent_loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| map_jni_error(env, "getClassLoader", e))?;
    let dex_class_loader_class = env
        .find_class("dalvik/system/DexClassLoader")
        .map_err(|e| map_jni_error(env, "find_class DexClassLoader", e))?;
    let class_loader = env
        .new_object(
            dex_class_loader_class,
            "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/ClassLoader;)V",
            &[
                JValue::Object(&dex_path_jstring),
                JValue::Object(&cache_path),
                JValue::Object(&JObject::null()),
                JValue::Object(&parent_loader),
            ],
        )
        .map_err(|e| map_jni_error(env, "new DexClassLoader", e))?;

    let global_ref = env
        .new_global_ref(class_loader)
        .map_err(|e| map_jni_error(env, "new_global_ref class_loader", e))?;
    let _ = CLASS_LOADER.set(global_ref);
    Ok(())
}

fn get_helper_class<'a>(env: &mut JNIEnv<'a>) -> Result<JClass<'a>, ContactsError> {
    let class_loader = CLASS_LOADER
        .get()
        .ok_or_else(|| ContactsError::Platform("class loader not initialized".into()))?;
    let helper_class_name = env
        .new_string("waterkit.contacts.ContactsHelper")
        .map_err(|e| map_jni_error(env, "new_string helper_class_name", e))?;
    let loaded_class = env
        .call_method(
            class_loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&helper_class_name)],
        )
        .and_then(jni::objects::JValueGen::l)
        .map_err(|e| map_jni_error(env, "loadClass ContactsHelper", e))?;
    Ok(loaded_class.into())
}

fn fetch_all_with_context(
    env: &mut JNIEnv<'_>,
    context: &JObject<'_>,
) -> Result<Vec<Contact>, ContactsError> {
    init_with_context(env, context)?;
    let helper_class = get_helper_class(env)?;
    let result = env
        .call_static_method(
            helper_class,
            "fetchAll",
            "(Landroid/content/Context;)[Ljava/lang/String;",
            &[JValue::Object(context)],
        )
        .map_err(|e| map_jni_error(env, "ContactsHelper.fetchAll", e))?
        .l()
        .map_err(|e| map_jni_error(env, "ContactsHelper.fetchAll result", e))?;
    parse_contacts_array(env, result)
}

fn search_with_context(
    env: &mut JNIEnv<'_>,
    context: &JObject<'_>,
    query: &str,
) -> Result<Vec<Contact>, ContactsError> {
    init_with_context(env, context)?;
    let helper_class = get_helper_class(env)?;
    let query_jstring = env
        .new_string(query)
        .map_err(|e| map_jni_error(env, "new_string query", e))?;
    let result = env
        .call_static_method(
            helper_class,
            "search",
            "(Landroid/content/Context;Ljava/lang/String;)[Ljava/lang/String;",
            &[JValue::Object(context), JValue::Object(&query_jstring)],
        )
        .map_err(|e| map_jni_error(env, "ContactsHelper.search", e))?
        .l()
        .map_err(|e| map_jni_error(env, "ContactsHelper.search result", e))?;
    parse_contacts_array(env, result)
}

fn get_with_context(
    env: &mut JNIEnv<'_>,
    context: &JObject<'_>,
    id: &str,
) -> Result<Contact, ContactsError> {
    init_with_context(env, context)?;
    let helper_class = get_helper_class(env)?;
    let id_jstring = env
        .new_string(id)
        .map_err(|e| map_jni_error(env, "new_string id", e))?;
    let result = env
        .call_static_method(
            helper_class,
            "getContact",
            "(Landroid/content/Context;Ljava/lang/String;)Ljava/lang/String;",
            &[JValue::Object(context), JValue::Object(&id_jstring)],
        )
        .map_err(|e| map_jni_error(env, "ContactsHelper.getContact", e))?
        .l()
        .map_err(|e| map_jni_error(env, "ContactsHelper.getContact result", e))?;
    if result.is_null() {
        return Err(ContactsError::NotFound(id.to_string()));
    }
    let line: String = env
        .get_string(&JString::from(result))
        .map_err(|e| map_jni_error(env, "ContactsHelper.getContact get_string", e))?
        .into();
    parse_contact_line(&line)
}

fn create_with_context(
    env: &mut JNIEnv<'_>,
    context: &JObject<'_>,
    data: &ContactData,
) -> Result<Contact, ContactsError> {
    init_with_context(env, context)?;
    let helper_class = get_helper_class(env)?;
    let payload = serialize_contact_data(data);
    let payload_jstring = env
        .new_string(payload)
        .map_err(|e| map_jni_error(env, "new_string payload", e))?;
    let result = env
        .call_static_method(
            helper_class,
            "createContact",
            "(Landroid/content/Context;Ljava/lang/String;)Ljava/lang/String;",
            &[JValue::Object(context), JValue::Object(&payload_jstring)],
        )
        .map_err(|e| map_jni_error(env, "ContactsHelper.createContact", e))?
        .l()
        .map_err(|e| map_jni_error(env, "ContactsHelper.createContact result", e))?;
    if result.is_null() {
        return Err(ContactsError::Platform(
            "createContact returned null".into(),
        ));
    }
    let line: String = env
        .get_string(&JString::from(result))
        .map_err(|e| map_jni_error(env, "ContactsHelper.createContact get_string", e))?
        .into();
    parse_contact_line(&line)
}

fn delete_with_context(
    env: &mut JNIEnv<'_>,
    context: &JObject<'_>,
    id: &str,
) -> Result<(), ContactsError> {
    init_with_context(env, context)?;
    let helper_class = get_helper_class(env)?;
    let id_jstring = env
        .new_string(id)
        .map_err(|e| map_jni_error(env, "new_string id", e))?;
    let deleted = env
        .call_static_method(
            helper_class,
            "deleteContact",
            "(Landroid/content/Context;Ljava/lang/String;)Z",
            &[JValue::Object(context), JValue::Object(&id_jstring)],
        )
        .map_err(|e| map_jni_error(env, "ContactsHelper.deleteContact", e))?
        .z()
        .map_err(|e| map_jni_error(env, "ContactsHelper.deleteContact result", e))?;
    if deleted {
        Ok(())
    } else {
        Err(ContactsError::NotFound(id.to_string()))
    }
}

fn parse_contacts_array(
    env: &mut JNIEnv<'_>,
    array_obj: JObject<'_>,
) -> Result<Vec<Contact>, ContactsError> {
    if array_obj.is_null() {
        return Err(ContactsError::Platform(
            "ContactsHelper returned null contact array".into(),
        ));
    }
    let array = JObjectArray::from(array_obj);
    let len_i32 = env
        .get_array_length(&array)
        .map_err(|e| map_jni_error(env, "get_array_length contacts", e))?;
    let len = usize::try_from(len_i32)
        .map_err(|_| ContactsError::Platform(format!("negative contacts len: {len_i32}")))?;

    let mut contacts = Vec::with_capacity(len);
    for idx in 0..len_i32 {
        let item = env
            .get_object_array_element(&array, idx)
            .map_err(|e| map_jni_error(env, "get_object_array_element contact", e))?;
        if item.is_null() {
            return Err(ContactsError::Platform(format!(
                "contact at index {idx} is null"
            )));
        }
        let line: String = env
            .get_string(&JString::from(item))
            .map_err(|e| map_jni_error(env, "get_string contact", e))?
            .into();
        contacts.push(parse_contact_line(&line)?);
    }
    Ok(contacts)
}

fn parse_contact_line(line: &str) -> Result<Contact, ContactsError> {
    let parts: Vec<&str> = line.split('\t').collect();
    let id = parts.first().copied().unwrap_or_default().to_string();
    if id.is_empty() {
        return Err(ContactsError::Platform(
            "contact payload missing id".into(),
        ));
    }

    Ok(Contact {
        id,
        given_name: parts
            .get(1)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string),
        family_name: parts
            .get(2)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string),
        organization: parts
            .get(3)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string),
        phone_numbers: parts
            .get(4)
            .unwrap_or(&"")
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|number| PhoneNumber {
                number: number.to_string(),
                label: None,
            })
            .collect(),
        email_addresses: parts
            .get(5)
            .unwrap_or(&"")
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|address| EmailAddress {
                address: address.to_string(),
                label: None,
            })
            .collect(),
        postal_addresses: Vec::new(),
        birthday: parts
            .get(6)
            .filter(|s| !s.is_empty())
            .and_then(|s| s.parse::<crate::Date>().ok()),
        note: parts
            .get(7)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string),
        thumbnail: None,
    })
}

fn serialize_contact_data(data: &ContactData) -> String {
    let phones = data
        .phone_numbers
        .iter()
        .map(|phone| phone.number.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let emails = data
        .email_addresses
        .iter()
        .map(|email| email.address.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let birthday = data
        .birthday
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default();
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
        data.given_name.as_deref().unwrap_or(""),
        data.family_name.as_deref().unwrap_or(""),
        data.organization.as_deref().unwrap_or(""),
        phones,
        emails,
        birthday,
        data.note.as_deref().unwrap_or(""),
    )
}

fn platform_error(action: &str, err: impl Display) -> ContactsError {
    let message = format!("{action}: {err}");
    if is_permission_message(&message) {
        ContactsError::PermissionDenied
    } else {
        ContactsError::Platform(message)
    }
}

fn map_jni_error(env: &mut JNIEnv<'_>, action: &str, err: impl Display) -> ContactsError {
    let mut message = format!("{action}: {err}");
    if let Some(java_exception) = take_java_exception_message(env) {
        if is_permission_message(&java_exception) {
            return ContactsError::PermissionDenied;
        }
        message = format!("{message} ({java_exception})");
    }
    if is_permission_message(&message) {
        ContactsError::PermissionDenied
    } else {
        ContactsError::Platform(message)
    }
}

fn take_java_exception_message(env: &mut JNIEnv<'_>) -> Option<String> {
    let has_exception = env.exception_check().ok()?;
    if !has_exception {
        return None;
    }
    let throwable = env.exception_occurred().ok()?;
    let _ = env.exception_clear();
    let rendered =
        if let Ok(value) = env.call_method(&throwable, "toString", "()Ljava/lang/String;", &[]) {
            value.l().ok()?
        } else {
            let _ = env.exception_clear();
            return Some("Java exception".into());
        };
    if rendered.is_null() {
        return Some("Java exception".into());
    }
    env.get_string(&JString::from(rendered))
        .ok()
        .map(Into::into)
}

fn is_permission_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("securityexception")
        || lower.contains("permission")
        || lower.contains("permission denial")
        || lower.contains("read_contacts")
        || lower.contains("write_contacts")
}

pub async fn fetch_all() -> Result<Vec<Contact>, ContactsError> {
    future::ready(with_android_context(fetch_all_with_context)).await
}

pub async fn search(query: &str) -> Result<Vec<Contact>, ContactsError> {
    future::ready(with_android_context(|env, context| {
        search_with_context(env, context, query)
    }))
    .await
}

pub async fn get(id: &str) -> Result<Contact, ContactsError> {
    future::ready(with_android_context(|env, context| {
        get_with_context(env, context, id)
    }))
    .await
}

pub async fn create(data: ContactData) -> Result<Contact, ContactsError> {
    future::ready(with_android_context(|env, context| {
        create_with_context(env, context, &data)
    }))
    .await
}

pub async fn delete(id: &str) -> Result<(), ContactsError> {
    future::ready(with_android_context(|env, context| {
        delete_with_context(env, context, id)
    }))
    .await
}
