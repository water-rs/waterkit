use crate::{Contact, ContactData, ContactsError, EmailAddress, PhoneNumber};
use futures::future;
use jni::objects::{JObject, JObjectArray, JString, JValue};
use jni::{Env, jni_sig, jni_str};
use std::fmt::Display;
use waterkit_build::{AndroidError, DexHelper, dex_helper, with_android_context};

/// `waterkit.contacts.ContactsHelper`, embedded as a DEX by this crate's build script and
/// loaded on first use.
static HELPER: DexHelper = dex_helper!("waterkit.contacts.ContactsHelper");

impl From<AndroidError> for ContactsError {
    fn from(error: AndroidError) -> Self {
        Self::Platform(error.to_string())
    }
}

fn decode_string(env: &mut Env<'_>, value: &JObject<'_>) -> Result<String, ContactsError> {
    match env
        .as_cast::<JString>(value)
        .and_then(|text| text.try_to_string(env))
    {
        Ok(text) => Ok(text),
        Err(error) => Err(map_jni_error(env, "decode string", error)),
    }
}

fn fetch_all_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<Vec<Contact>, ContactsError> {
    let helper_class = HELPER.class(env, context)?;
    let result = env
        .call_static_method(
            helper_class,
            jni_str!("fetchAll"),
            jni_sig!("(Landroid/content/Context;)[Ljava/lang/String;"),
            &[JValue::Object(context)],
        )
        .map_err(|e| map_jni_error(env, "ContactsHelper.fetchAll", e))?
        .l()
        .map_err(|e| map_jni_error(env, "ContactsHelper.fetchAll result", e))?;
    parse_contacts_array(env, result)
}

fn search_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    query: &str,
) -> Result<Vec<Contact>, ContactsError> {
    let helper_class = HELPER.class(env, context)?;
    let query_jstring = env
        .new_string(query)
        .map_err(|e| map_jni_error(env, "new_string query", e))?;
    let result = env
        .call_static_method(
            helper_class,
            jni_str!("search"),
            jni_sig!("(Landroid/content/Context;Ljava/lang/String;)[Ljava/lang/String;"),
            &[JValue::Object(context), JValue::Object(&query_jstring)],
        )
        .map_err(|e| map_jni_error(env, "ContactsHelper.search", e))?
        .l()
        .map_err(|e| map_jni_error(env, "ContactsHelper.search result", e))?;
    parse_contacts_array(env, result)
}

fn get_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    id: &str,
) -> Result<Contact, ContactsError> {
    let helper_class = HELPER.class(env, context)?;
    let id_jstring = env
        .new_string(id)
        .map_err(|e| map_jni_error(env, "new_string id", e))?;
    let result = env
        .call_static_method(
            helper_class,
            jni_str!("getContact"),
            jni_sig!("(Landroid/content/Context;Ljava/lang/String;)Ljava/lang/String;"),
            &[JValue::Object(context), JValue::Object(&id_jstring)],
        )
        .map_err(|e| map_jni_error(env, "ContactsHelper.getContact", e))?
        .l()
        .map_err(|e| map_jni_error(env, "ContactsHelper.getContact result", e))?;
    if result.is_null() {
        return Err(ContactsError::NotFound(id.to_string()));
    }
    let line = decode_string(env, &result)?;
    parse_contact_line(&line)
}

fn create_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    data: &ContactData,
) -> Result<Contact, ContactsError> {
    let helper_class = HELPER.class(env, context)?;
    let payload = serialize_contact_data(data);
    let payload_jstring = env
        .new_string(payload)
        .map_err(|e| map_jni_error(env, "new_string payload", e))?;
    let result = env
        .call_static_method(
            helper_class,
            jni_str!("createContact"),
            jni_sig!("(Landroid/content/Context;Ljava/lang/String;)Ljava/lang/String;"),
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
    let line = decode_string(env, &result)?;
    parse_contact_line(&line)
}

fn delete_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    id: &str,
) -> Result<(), ContactsError> {
    let helper_class = HELPER.class(env, context)?;
    let id_jstring = env
        .new_string(id)
        .map_err(|e| map_jni_error(env, "new_string id", e))?;
    let deleted = env
        .call_static_method(
            helper_class,
            jni_str!("deleteContact"),
            jni_sig!("(Landroid/content/Context;Ljava/lang/String;)Z"),
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
    env: &mut Env<'_>,
    array_obj: JObject<'_>,
) -> Result<Vec<Contact>, ContactsError> {
    if array_obj.is_null() {
        return Err(ContactsError::Platform(
            "ContactsHelper returned null contact array".into(),
        ));
    }
    let array = env
        .cast_local::<JObjectArray>(array_obj)
        .map_err(|e| map_jni_error(env, "cast contact array", e))?;
    let len = array
        .len(env)
        .map_err(|e| map_jni_error(env, "contact array length", e))?;

    let mut contacts = Vec::with_capacity(len);
    for idx in 0..len {
        let item = array
            .get_element(env, idx)
            .map_err(|e| map_jni_error(env, "contact array element", e))?;
        if item.is_null() {
            return Err(ContactsError::Platform(format!(
                "contact at index {idx} is null"
            )));
        }
        let line = decode_string(env, &item)?;
        contacts.push(parse_contact_line(&line)?);
    }
    Ok(contacts)
}

fn parse_contact_line(line: &str) -> Result<Contact, ContactsError> {
    let parts: Vec<&str> = line.split('\t').collect();
    let id = parts.first().copied().unwrap_or_default().to_string();
    if id.is_empty() {
        return Err(ContactsError::Platform("contact payload missing id".into()));
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

fn map_jni_error(env: &mut Env<'_>, action: &str, err: impl Display) -> ContactsError {
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

fn take_java_exception_message(env: &mut Env<'_>) -> Option<String> {
    if !env.exception_check() {
        return None;
    }
    let throwable = env.exception_occurred()?;
    env.exception_clear();

    let rendered = env
        .call_method(
            &throwable,
            jni_str!("toString"),
            jni_sig!("()Ljava/lang/String;"),
            &[],
        )
        .and_then(jni::objects::JValueOwned::l);
    let Ok(rendered) = rendered else {
        env.exception_clear();
        return Some("Java exception".into());
    };
    if rendered.is_null() {
        return Some("Java exception".into());
    }

    env.as_cast::<JString>(&rendered)
        .and_then(|text| text.try_to_string(env))
        .ok()
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
