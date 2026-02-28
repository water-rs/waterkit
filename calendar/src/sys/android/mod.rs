use crate::{Calendar, CalendarError, Event, EventData};
use futures::future;
use jni::JNIEnv;
use jni::errors::Error as JniError;
use jni::objects::{GlobalRef, JClass, JObject, JObjectArray, JString, JValue};
use std::mem::ManuallyDrop;
use std::sync::OnceLock;

const HELPER_CLASS_NAME: &str = "waterkit.calendar.CalendarHelper";
static DEX_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/classes.dex"));
static CLASS_LOADER: OnceLock<GlobalRef> = OnceLock::new();

fn with_android_context<T, F>(f: F) -> Result<T, CalendarError>
where
    F: for<'local> FnOnce(&mut JNIEnv<'local>, &JObject<'local>) -> Result<T, CalendarError>,
{
    let android_context = ndk_context::android_context();
    let vm = unsafe { jni::JavaVM::from_raw(android_context.vm().cast()) }.map_err(|error| {
        CalendarError::PlatformError(format!("JavaVM::from_raw failed: {error}"))
    })?;
    let mut env = vm.attach_current_thread().map_err(|error| {
        CalendarError::PlatformError(format!("attach_current_thread failed: {error}"))
    })?;

    let context = ManuallyDrop::new(unsafe { JObject::from_raw(android_context.context().cast()) });
    assert!(
        !context.is_null(),
        "waterkit-calendar: ndk_context returned null Android Context"
    );

    f(&mut env, &context)
}

fn init_dex(env: &mut JNIEnv, context: &JObject) -> Result<(), CalendarError> {
    if CLASS_LOADER.get().is_some() {
        return Ok(());
    }

    let cache_dir = env
        .call_method(context, "getCacheDir", "()Ljava/io/File;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|error| map_jni_error(env, "Context.getCacheDir failed", &error))?;

    let cache_path = env
        .call_method(&cache_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|error| map_jni_error(env, "File.getAbsolutePath failed", &error))?;

    let cache_path_string: String = env
        .get_string(&JString::from(cache_path))
        .map_err(|error| {
            CalendarError::PlatformError(format!("cache path decode failed: {error}"))
        })?
        .into();

    let dex_path = format!("{cache_path_string}/waterkit_calendar.dex");
    let _ = std::fs::remove_file(&dex_path);
    std::fs::write(&dex_path, DEX_BYTES)
        .map_err(|error| CalendarError::PlatformError(format!("write DEX failed: {error}")))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = std::fs::metadata(&dex_path)
            .map_err(|error| CalendarError::PlatformError(format!("dex metadata failed: {error}")))?
            .permissions();
        permissions.set_mode(0o444);
        std::fs::set_permissions(&dex_path, permissions).map_err(|error| {
            CalendarError::PlatformError(format!("set dex permissions failed: {error}"))
        })?;
    }

    let dex_path_java = env.new_string(dex_path).map_err(|error| {
        CalendarError::PlatformError(format!("new dex path string failed: {error}"))
    })?;
    let cache_path_java = env.new_string(cache_path_string).map_err(|error| {
        CalendarError::PlatformError(format!("new cache path string failed: {error}"))
    })?;

    let parent_loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
        .and_then(jni::objects::JValueGen::l)
        .map_err(|error| map_jni_error(env, "Context.getClassLoader failed", &error))?;

    let dex_loader_class = env
        .find_class("dalvik/system/DexClassLoader")
        .map_err(|error| map_jni_error(env, "find DexClassLoader failed", &error))?;

    let class_loader = env
        .new_object(
            dex_loader_class,
            "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/ClassLoader;)V",
            &[
                JValue::Object(&dex_path_java),
                JValue::Object(&cache_path_java),
                JValue::Object(&JObject::null()),
                JValue::Object(&parent_loader),
            ],
        )
        .map_err(|error| map_jni_error(env, "new DexClassLoader failed", &error))?;

    let class_loader_global = env
        .new_global_ref(class_loader)
        .map_err(|error| CalendarError::PlatformError(format!("new_global_ref failed: {error}")))?;

    if CLASS_LOADER.set(class_loader_global).is_err() {
        assert!(
            CLASS_LOADER.get().is_some(),
            "calendar class loader initialization race left loader unset"
        );
    }

    Ok(())
}

fn get_helper_class<'local>(env: &mut JNIEnv<'local>) -> Result<JClass<'local>, CalendarError> {
    let class_loader = CLASS_LOADER
        .get()
        .ok_or_else(|| CalendarError::PlatformError("class loader not initialized".into()))?;

    let helper_name = env.new_string(HELPER_CLASS_NAME).map_err(|error| {
        CalendarError::PlatformError(format!("new helper class string failed: {error}"))
    })?;

    let helper_class = env
        .call_method(
            class_loader.as_obj(),
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&helper_name)],
        )
        .and_then(jni::objects::JValueGen::l)
        .map_err(|error| map_jni_error(env, "ClassLoader.loadClass failed", &error))?;

    Ok(helper_class.into())
}

fn ensure_calendar_permission(
    env: &mut JNIEnv,
    context: &JObject,
    write_access: bool,
) -> Result<(), CalendarError> {
    let helper_class = get_helper_class(env)?;
    let write_flag = u8::from(write_access);
    let granted = env
        .call_static_method(
            &helper_class,
            "hasCalendarPermission",
            "(Landroid/content/Context;Z)Z",
            &[JValue::Object(context), JValue::Bool(write_flag)],
        )
        .map_err(|error| map_jni_error(env, "CalendarHelper.hasCalendarPermission failed", &error))?
        .z()
        .map_err(|error| {
            CalendarError::PlatformError(format!(
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
    env: &mut JNIEnv,
    array_object: JObject,
    op_name: &str,
) -> Result<Vec<String>, CalendarError> {
    if array_object.is_null() {
        return Ok(Vec::new());
    }

    let array = JObjectArray::from(array_object);
    let len = env.get_array_length(&array).map_err(|error| {
        CalendarError::PlatformError(format!("{op_name}: get_array_length failed: {error}"))
    })?;
    let len_usize = usize::try_from(len).map_err(|_| {
        CalendarError::PlatformError(format!("{op_name}: negative array length returned: {len}"))
    })?;
    let mut rows = Vec::with_capacity(len_usize);
    for index in 0..len {
        let value = env
            .get_object_array_element(&array, index)
            .map_err(|error| {
                CalendarError::PlatformError(format!(
                    "{op_name}: get_object_array_element({index}) failed: {error}"
                ))
            })?;
        if value.is_null() {
            continue;
        }

        let decoded: String = env
            .get_string(&JString::from(value))
            .map_err(|error| {
                CalendarError::PlatformError(format!(
                    "{op_name}: get_string({index}) failed: {error}"
                ))
            })?
            .into();
        rows.push(decoded);
    }

    Ok(rows)
}

fn parse_calendar_row(row: &str) -> Result<Calendar, CalendarError> {
    let parts: Vec<&str> = row.splitn(4, '\t').collect();
    if parts.len() != 4 {
        return Err(CalendarError::PlatformError(format!(
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
        return Err(CalendarError::PlatformError(format!(
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
        start_date: parts[4].to_string(),
        end_date: parts[5].to_string(),
        is_all_day: parts[6] == "1",
        calendar_id: parts[7].to_string(),
    })
}

fn list_calendars_with_context(
    env: &mut JNIEnv,
    context: &JObject,
) -> Result<Vec<Calendar>, CalendarError> {
    init_dex(env, context)?;
    ensure_calendar_permission(env, context, false)?;

    let helper_class = get_helper_class(env)?;
    let rows_object = env
        .call_static_method(
            &helper_class,
            "listCalendars",
            "(Landroid/content/Context;)[Ljava/lang/String;",
            &[JValue::Object(context)],
        )
        .map_err(|error| map_jni_error(env, "CalendarHelper.listCalendars failed", &error))?
        .l()
        .map_err(|error| {
            CalendarError::PlatformError(format!(
                "CalendarHelper.listCalendars result decode failed: {error}"
            ))
        })?;

    let rows = read_string_array(env, rows_object, "CalendarHelper.listCalendars")?;
    rows.iter().map(|row| parse_calendar_row(row)).collect()
}

fn fetch_events_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    start: &str,
    end: &str,
) -> Result<Vec<Event>, CalendarError> {
    init_dex(env, context)?;
    ensure_calendar_permission(env, context, false)?;

    let helper_class = get_helper_class(env)?;
    let start_java = env.new_string(start).map_err(|error| {
        CalendarError::PlatformError(format!("new start string failed: {error}"))
    })?;
    let end_java = env
        .new_string(end)
        .map_err(|error| CalendarError::PlatformError(format!("new end string failed: {error}")))?;

    let rows_object = env
        .call_static_method(
            &helper_class,
            "fetchEvents",
            "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)[Ljava/lang/String;",
            &[
                JValue::Object(context),
                JValue::Object(&start_java),
                JValue::Object(&end_java),
            ],
        )
        .map_err(|error| map_jni_error(env, "CalendarHelper.fetchEvents failed", &error))?
        .l()
        .map_err(|error| {
            CalendarError::PlatformError(format!(
                "CalendarHelper.fetchEvents result decode failed: {error}"
            ))
        })?;

    let rows = read_string_array(env, rows_object, "CalendarHelper.fetchEvents")?;
    rows.iter().map(|row| parse_event_row(row)).collect()
}

fn create_event_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    data: &EventData,
) -> Result<Event, CalendarError> {
    init_dex(env, context)?;
    ensure_calendar_permission(env, context, true)?;

    let helper_class = get_helper_class(env)?;
    let title_java = env.new_string(&data.title).map_err(|error| {
        CalendarError::PlatformError(format!("new title string failed: {error}"))
    })?;
    let notes_java = env
        .new_string(data.notes.as_deref().unwrap_or(""))
        .map_err(|error| {
            CalendarError::PlatformError(format!("new notes string failed: {error}"))
        })?;
    let location_java = env
        .new_string(data.location.as_deref().unwrap_or(""))
        .map_err(|error| {
            CalendarError::PlatformError(format!("new location string failed: {error}"))
        })?;
    let start_java = env.new_string(&data.start_date).map_err(|error| {
        CalendarError::PlatformError(format!("new start date string failed: {error}"))
    })?;
    let end_java = env.new_string(&data.end_date).map_err(|error| {
        CalendarError::PlatformError(format!("new end date string failed: {error}"))
    })?;
    let calendar_id_java = env
        .new_string(data.calendar_id.as_deref().unwrap_or(""))
        .map_err(|error| {
            CalendarError::PlatformError(format!("new calendar id string failed: {error}"))
        })?;

    let created_event_id = env
        .call_static_method(
            &helper_class,
            "createEventIso",
            "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;ZLjava/lang/String;)J",
            &[
                JValue::Object(context),
                JValue::Object(&title_java),
                JValue::Object(&notes_java),
                JValue::Object(&location_java),
                JValue::Object(&start_java),
                JValue::Object(&end_java),
                JValue::Bool(u8::from(data.is_all_day)),
                JValue::Object(&calendar_id_java),
            ],
        )
        .map_err(|error| map_jni_error(env, "CalendarHelper.createEventIso failed", &error))?
        .j()
        .map_err(|error| {
            CalendarError::PlatformError(format!("CalendarHelper.createEventIso result decode failed: {error}"))
        })?;

    if created_event_id < 0 {
        return Err(CalendarError::PlatformError(
            "CalendarHelper.createEventIso returned invalid event id".into(),
        ));
    }

    let created_event_row = env
        .call_static_method(
            &helper_class,
            "fetchEventById",
            "(Landroid/content/Context;J)Ljava/lang/String;",
            &[JValue::Object(context), JValue::Long(created_event_id)],
        )
        .map_err(|error| map_jni_error(env, "CalendarHelper.fetchEventById failed", &error))?
        .l()
        .map_err(|error| {
            CalendarError::PlatformError(format!(
                "CalendarHelper.fetchEventById result decode failed: {error}"
            ))
        })?;

    if created_event_row.is_null() {
        return Err(CalendarError::PlatformError(format!(
            "CalendarHelper.fetchEventById returned null for created event {created_event_id}"
        )));
    }

    let created_event_row: String = env
        .get_string(&JString::from(created_event_row))
        .map_err(|error| {
            CalendarError::PlatformError(format!(
                "CalendarHelper.fetchEventById row decode failed: {error}"
            ))
        })?
        .into();
    parse_event_row(&created_event_row)
}

fn delete_event_with_context(
    env: &mut JNIEnv,
    context: &JObject,
    id: &str,
) -> Result<(), CalendarError> {
    let event_id = id
        .parse::<i64>()
        .map_err(|_| CalendarError::NotFound(id.to_string()))?;

    init_dex(env, context)?;
    ensure_calendar_permission(env, context, true)?;

    let helper_class = get_helper_class(env)?;
    let deleted = env
        .call_static_method(
            &helper_class,
            "deleteEvent",
            "(Landroid/content/Context;J)Z",
            &[JValue::Object(context), JValue::Long(event_id)],
        )
        .map_err(|error| map_jni_error(env, "CalendarHelper.deleteEvent failed", &error))?
        .z()
        .map_err(|error| {
            CalendarError::PlatformError(format!(
                "CalendarHelper.deleteEvent result decode failed: {error}"
            ))
        })?;

    if deleted {
        Ok(())
    } else {
        Err(CalendarError::NotFound(id.to_string()))
    }
}

fn map_jni_error(env: &mut JNIEnv<'_>, operation: &str, error: &JniError) -> CalendarError {
    let message = take_java_exception_string(env).map_or_else(
        || format!("{operation}: {error}"),
        |java_message| format!("{operation}: {java_message}"),
    );

    if is_permission_denied_message(&message) {
        CalendarError::PermissionDenied
    } else {
        CalendarError::PlatformError(message)
    }
}

fn take_java_exception_string(env: &mut JNIEnv<'_>) -> Option<String> {
    let has_exception = env.exception_check().ok()?;
    if !has_exception {
        return None;
    }

    let throwable = env.exception_occurred().ok()?;
    let _ = env.exception_clear();

    let throwable_text = env
        .call_method(throwable, "toString", "()Ljava/lang/String;", &[])
        .ok()?
        .l()
        .ok()?;
    if throwable_text.is_null() {
        return Some("java.lang.Throwable".into());
    }

    env.get_string(&JString::from(throwable_text))
        .ok()
        .map(Into::into)
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
    future::ready(with_android_context(|env, context| {
        list_calendars_with_context(env, context)
    }))
    .await
}

pub async fn fetch_events(start: &str, end: &str) -> Result<Vec<Event>, CalendarError> {
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
