use crate::{Calendar, CalendarError, Event, EventData};
use futures::future;
use jni::errors::Error as JniError;
use jni::objects::{Global, JClass, JObject, JObjectArray, JString, JValue};
use jni::{Env, JavaVM, jni_sig, jni_str};
use std::sync::OnceLock;
use waterkit_core::Timestamp;

const HELPER_CLASS_NAME: &str = "waterkit.calendar.CalendarHelper";
static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));

/// [`HELPER_CLASS_NAME`], loaded once from [`DEX_BYTES`].
static HELPER_CLASS: OnceLock<Global<JClass<'static>>> = OnceLock::new();

fn with_android_context<T, F>(f: F) -> Result<T, CalendarError>
where
    F: FnOnce(&mut Env<'_>, &JObject<'_>) -> Result<T, CalendarError>,
{
    let android_context = ndk_context::android_context();
    let raw_vm: *mut jni::sys::JavaVM = android_context.vm().cast();
    let raw_context: jni::sys::jobject = android_context.context().cast();
    assert!(
        !raw_vm.is_null(),
        "waterkit-calendar: ndk_context returned a null JavaVM"
    );
    assert!(
        !raw_context.is_null(),
        "waterkit-calendar: ndk_context returned a null Android Context"
    );

    // SAFETY: `ndk_context` publishes the process' JavaVM pointer, which stays
    // valid for the lifetime of the application.
    let vm = unsafe { JavaVM::from_raw(raw_vm) };
    vm.attach_current_thread(
        |env| -> Result<Result<T, CalendarError>, jni::errors::Error> {
            // SAFETY: `ndk_context` publishes a global reference to the application
            // `Context` that outlives this attachment, and `as_cast_raw` only
            // borrows it.
            let context = unsafe { env.as_cast_raw::<JObject>(&raw_context)? };
            Ok(f(env, &context))
        },
    )
    .map_err(|error| CalendarError::Platform(format!("attach_current_thread failed: {error}")))?
}

/// Returns the cached helper class, loading the embedded DEX on first use.
fn helper_class(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<&'static Global<JClass<'static>>, CalendarError> {
    if let Some(class) = HELPER_CLASS.get() {
        return Ok(class);
    }

    let class = load_helper_class(env, context)?;
    Ok(HELPER_CLASS.get_or_init(|| class))
}

fn load_helper_class(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<Global<JClass<'static>>, CalendarError> {
    let parent_loader = env
        .call_method(
            context,
            jni_str!("getClassLoader"),
            jni_sig!("()Ljava/lang/ClassLoader;"),
            &[],
        )
        .and_then(jni::objects::JValueOwned::l)
        .map_err(|error| map_jni_error(env, "Context.getClassLoader failed", &error))?;

    let dex_bytes = env
        .byte_array_from_slice(DEX_BYTES)
        .map_err(|error| CalendarError::Platform(format!("copy DEX failed: {error}")))?;
    let dex_bytes = JObject::from(dex_bytes);
    let dex_buffer = env
        .call_static_method(
            jni_str!("java/nio/ByteBuffer"),
            jni_str!("wrap"),
            jni_sig!("([B)Ljava/nio/ByteBuffer;"),
            &[JValue::Object(&dex_bytes)],
        )
        .and_then(jni::objects::JValueOwned::l)
        .map_err(|error| map_jni_error(env, "ByteBuffer.wrap DEX failed", &error))?;
    let class_loader = env
        .new_object(
            jni_str!("dalvik/system/InMemoryDexClassLoader"),
            jni_sig!("(Ljava/nio/ByteBuffer;Ljava/lang/ClassLoader;)V"),
            &[JValue::Object(&dex_buffer), JValue::Object(&parent_loader)],
        )
        .map_err(|error| map_jni_error(env, "new InMemoryDexClassLoader failed", &error))?;

    let helper_name = env.new_string(HELPER_CLASS_NAME).map_err(|error| {
        CalendarError::Platform(format!("new helper class string failed: {error}"))
    })?;
    let helper_class = env
        .call_method(
            &class_loader,
            jni_str!("loadClass"),
            jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
            &[JValue::Object(&helper_name)],
        )
        .and_then(jni::objects::JValueOwned::l)
        .map_err(|error| map_jni_error(env, "ClassLoader.loadClass failed", &error))?;
    let helper_class = env.cast_local::<JClass>(helper_class).map_err(|error| {
        CalendarError::Platform(format!("loadClass returned a non-class: {error}"))
    })?;

    env.new_global_ref(helper_class)
        .map_err(|error| CalendarError::Platform(format!("new_global_ref failed: {error}")))
}

fn decode_string(env: &Env<'_>, value: &JObject<'_>) -> Result<String, CalendarError> {
    match env
        .as_cast::<JString>(value)
        .and_then(|text| text.try_to_string(env))
    {
        Ok(text) => Ok(text),
        Err(error) => Err(CalendarError::Platform(format!(
            "string decode failed: {error}"
        ))),
    }
}

fn ensure_calendar_permission(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    write_access: bool,
) -> Result<(), CalendarError> {
    let helper_class = helper_class(env, context)?;
    let granted = env
        .call_static_method(
            helper_class,
            jni_str!("hasCalendarPermission"),
            jni_sig!("(Landroid/content/Context;Z)Z"),
            &[JValue::Object(context), JValue::Bool(write_access)],
        )
        .map_err(|error| map_jni_error(env, "CalendarHelper.hasCalendarPermission failed", &error))?
        .z()
        .map_err(|error| {
            CalendarError::Platform(format!(
                "hasCalendarPermission result decode failed: {error}"
            ))
        })?;

    if granted {
        Ok(())
    } else {
        Err(CalendarError::PermissionDenied)
    }
}

fn read_string_array(
    env: &mut Env<'_>,
    array_object: JObject<'_>,
    op_name: &str,
) -> Result<Vec<String>, CalendarError> {
    if array_object.is_null() {
        return Ok(Vec::new());
    }

    let array = env
        .cast_local::<JObjectArray>(array_object)
        .map_err(|error| {
            CalendarError::Platform(format!("{op_name}: result is not an array: {error}"))
        })?;
    let len = array.len(env).map_err(|error| {
        CalendarError::Platform(format!("{op_name}: array length failed: {error}"))
    })?;

    let mut rows = Vec::with_capacity(len);
    for index in 0..len {
        let value = array.get_element(env, index).map_err(|error| {
            CalendarError::Platform(format!("{op_name}: element({index}) failed: {error}"))
        })?;
        if value.is_null() {
            continue;
        }

        rows.push(decode_string(env, &value)?);
    }

    Ok(rows)
}

fn parse_calendar_row(row: &str) -> Result<Calendar, CalendarError> {
    let parts: Vec<&str> = row.splitn(4, '\t').collect();
    if parts.len() != 4 {
        return Err(CalendarError::Platform(format!(
            "malformed calendar row (expected 4 fields): {row}"
        )));
    }

    Ok(Calendar {
        id: parts[0].to_string(),
        title: parts[1].to_string(),
        color: if parts[2].is_empty() {
            None
        } else {
            Some(parts[2].to_string())
        },
        is_read_only: parts[3] == "1",
    })
}

fn parse_event_row(row: &str) -> Result<Event, CalendarError> {
    let parts: Vec<&str> = row.splitn(8, '\t').collect();
    if parts.len() != 8 {
        return Err(CalendarError::Platform(format!(
            "malformed event row (expected 8 fields): {row}"
        )));
    }

    Ok(Event {
        id: parts[0].to_string(),
        title: parts[1].to_string(),
        notes: if parts[2].is_empty() {
            None
        } else {
            Some(parts[2].to_string())
        },
        location: if parts[3].is_empty() {
            None
        } else {
            Some(parts[3].to_string())
        },
        start: parts[4]
            .parse::<Timestamp>()
            .map_err(|error| CalendarError::Platform(format!("invalid start: {error}")))?,
        end: parts[5]
            .parse::<Timestamp>()
            .map_err(|error| CalendarError::Platform(format!("invalid end: {error}")))?,
        is_all_day: parts[6] == "1",
        calendar_id: parts[7].to_string(),
    })
}

fn list_calendars_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<Vec<Calendar>, CalendarError> {
    ensure_calendar_permission(env, context, false)?;

    let helper_class = helper_class(env, context)?;
    let rows_object = env
        .call_static_method(
            helper_class,
            jni_str!("listCalendars"),
            jni_sig!("(Landroid/content/Context;)[Ljava/lang/String;"),
            &[JValue::Object(context)],
        )
        .map_err(|error| map_jni_error(env, "CalendarHelper.listCalendars failed", &error))?
        .l()
        .map_err(|error| {
            CalendarError::Platform(format!(
                "CalendarHelper.listCalendars result decode failed: {error}"
            ))
        })?;

    let rows = read_string_array(env, rows_object, "CalendarHelper.listCalendars")?;
    rows.iter().map(|row| parse_calendar_row(row)).collect()
}

fn fetch_events_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    start: Timestamp,
    end: Timestamp,
) -> Result<Vec<Event>, CalendarError> {
    ensure_calendar_permission(env, context, false)?;

    let helper_class = helper_class(env, context)?;
    let start_str = start.to_string();
    let end_str = end.to_string();
    let start_java = env
        .new_string(&start_str)
        .map_err(|error| CalendarError::Platform(format!("new start string failed: {error}")))?;
    let end_java = env
        .new_string(&end_str)
        .map_err(|error| CalendarError::Platform(format!("new end string failed: {error}")))?;

    let rows_object = env
        .call_static_method(
            helper_class,
            jni_str!("fetchEvents"),
            jni_sig!(
                "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)[Ljava/lang/String;"
            ),
            &[
                JValue::Object(context),
                JValue::Object(&start_java),
                JValue::Object(&end_java),
            ],
        )
        .map_err(|error| map_jni_error(env, "CalendarHelper.fetchEvents failed", &error))?
        .l()
        .map_err(|error| {
            CalendarError::Platform(format!(
                "CalendarHelper.fetchEvents result decode failed: {error}"
            ))
        })?;

    let rows = read_string_array(env, rows_object, "CalendarHelper.fetchEvents")?;
    rows.iter().map(|row| parse_event_row(row)).collect()
}

fn create_event_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    data: &EventData,
) -> Result<Event, CalendarError> {
    ensure_calendar_permission(env, context, true)?;

    let helper_class = helper_class(env, context)?;
    let title_java = env
        .new_string(&data.title)
        .map_err(|error| CalendarError::Platform(format!("new title string failed: {error}")))?;
    let notes_java = env
        .new_string(data.notes.as_deref().unwrap_or(""))
        .map_err(|error| CalendarError::Platform(format!("new notes string failed: {error}")))?;
    let location_java = env
        .new_string(data.location.as_deref().unwrap_or(""))
        .map_err(|error| CalendarError::Platform(format!("new location string failed: {error}")))?;
    let start_str = data.start.to_string();
    let end_str = data.end.to_string();
    let start_java = env.new_string(&start_str).map_err(|error| {
        CalendarError::Platform(format!("new start date string failed: {error}"))
    })?;
    let end_java = env
        .new_string(&end_str)
        .map_err(|error| CalendarError::Platform(format!("new end date string failed: {error}")))?;
    let calendar_id_java = env
        .new_string(data.calendar_id.as_deref().unwrap_or(""))
        .map_err(|error| {
            CalendarError::Platform(format!("new calendar id string failed: {error}"))
        })?;

    let created_event_id = env
        .call_static_method(
            helper_class,
            jni_str!("createEventIso"),
            jni_sig!("(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;ZLjava/lang/String;)J"),
            &[
                JValue::Object(context),
                JValue::Object(&title_java),
                JValue::Object(&notes_java),
                JValue::Object(&location_java),
                JValue::Object(&start_java),
                JValue::Object(&end_java),
                JValue::Bool(data.is_all_day),
                JValue::Object(&calendar_id_java),
            ],
        )
        .map_err(|error| map_jni_error(env, "CalendarHelper.createEventIso failed", &error))?
        .j()
        .map_err(|error| {
            CalendarError::Platform(format!("CalendarHelper.createEventIso result decode failed: {error}"))
        })?;

    if created_event_id < 0 {
        return Err(CalendarError::Platform(
            "CalendarHelper.createEventIso returned invalid event id".into(),
        ));
    }

    let created_event_row = env
        .call_static_method(
            helper_class,
            jni_str!("fetchEventById"),
            jni_sig!("(Landroid/content/Context;J)Ljava/lang/String;"),
            &[JValue::Object(context), JValue::Long(created_event_id)],
        )
        .map_err(|error| map_jni_error(env, "CalendarHelper.fetchEventById failed", &error))?
        .l()
        .map_err(|error| {
            CalendarError::Platform(format!(
                "CalendarHelper.fetchEventById result decode failed: {error}"
            ))
        })?;

    if created_event_row.is_null() {
        return Err(CalendarError::Platform(format!(
            "CalendarHelper.fetchEventById returned null for created event {created_event_id}"
        )));
    }

    let created_event_row = decode_string(env, &created_event_row)?;
    parse_event_row(&created_event_row)
}

fn delete_event_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
    id: &str,
) -> Result<(), CalendarError> {
    let event_id = id
        .parse::<i64>()
        .map_err(|_| CalendarError::NotFound(id.to_string()))?;

    ensure_calendar_permission(env, context, true)?;

    let helper_class = helper_class(env, context)?;
    let deleted = env
        .call_static_method(
            helper_class,
            jni_str!("deleteEvent"),
            jni_sig!("(Landroid/content/Context;J)Z"),
            &[JValue::Object(context), JValue::Long(event_id)],
        )
        .map_err(|error| map_jni_error(env, "CalendarHelper.deleteEvent failed", &error))?
        .z()
        .map_err(|error| {
            CalendarError::Platform(format!(
                "CalendarHelper.deleteEvent result decode failed: {error}"
            ))
        })?;

    if deleted {
        Ok(())
    } else {
        Err(CalendarError::NotFound(id.to_string()))
    }
}

fn map_jni_error(env: &mut Env<'_>, operation: &str, error: &JniError) -> CalendarError {
    let message = take_java_exception_string(env).map_or_else(
        || format!("{operation}: {error}"),
        |java_message| format!("{operation}: {java_message}"),
    );

    if is_permission_denied_message(&message) {
        CalendarError::PermissionDenied
    } else {
        CalendarError::Platform(message)
    }
}

fn take_java_exception_string(env: &mut Env<'_>) -> Option<String> {
    if !env.exception_check() {
        return None;
    }

    let throwable = env.exception_occurred()?;
    env.exception_clear();

    let throwable_text = env
        .call_method(
            &throwable,
            jni_str!("toString"),
            jni_sig!("()Ljava/lang/String;"),
            &[],
        )
        .ok()?
        .l()
        .ok()?;
    if throwable_text.is_null() {
        return Some("java.lang.Throwable".into());
    }

    env.as_cast::<JString>(&throwable_text)
        .and_then(|text| text.try_to_string(env))
        .ok()
}

fn is_permission_denied_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("securityexception")
        || message.contains("permission denial")
        || message.contains("permission denied")
        || message.contains("android.permission.read_calendar")
        || message.contains("android.permission.write_calendar")
}

pub async fn list_calendars() -> Result<Vec<Calendar>, CalendarError> {
    future::ready(with_android_context(list_calendars_with_context)).await
}

pub async fn fetch_events(start: Timestamp, end: Timestamp) -> Result<Vec<Event>, CalendarError> {
    future::ready(with_android_context(|env, context| {
        fetch_events_with_context(env, context, start, end)
    }))
    .await
}

pub async fn create_event(data: EventData) -> Result<Event, CalendarError> {
    future::ready(with_android_context(|env, context| {
        create_event_with_context(env, context, &data)
    }))
    .await
}

pub async fn delete_event(id: &str) -> Result<(), CalendarError> {
    future::ready(with_android_context(|env, context| {
        delete_event_with_context(env, context, id)
    }))
    .await
}
