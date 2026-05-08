use crate::{ShareError, ShareResult, ShareSheet};
use futures::future;
use jni::JNIEnv;
use jni::JavaVM;
use jni::objects::JObject;
use std::mem::ManuallyDrop;

fn with_android_context<T, F>(f: F) -> Result<T, ShareError>
where
    F: for<'local> FnOnce(&mut JNIEnv<'local>, &JObject<'local>) -> Result<T, ShareError>,
{
    let android_context = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(android_context.vm().cast()) }
        .map_err(|e| ShareError::Platform(format!("JavaVM::from_raw: {e}")))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| ShareError::Platform(format!("attach_current_thread: {e}")))?;

    let context = ManuallyDrop::new(unsafe { JObject::from_raw(android_context.context().cast()) });
    assert!(
        !context.is_null(),
        "ndk_context returned null Android Context"
    );

    f(&mut env, &context)
}

pub async fn show_share_sheet(sheet: ShareSheet) -> Result<ShareResult, ShareError> {
    future::ready(with_android_context(|env, context| {
        jni_api::share_with_context(env, context, &sheet)
    }))
    .await
}

/// Android-specific share functions requiring JNI context.
pub mod jni_api {
    use crate::{ShareError, ShareItem, ShareResult, ShareSheet};
    use jni::JNIEnv;
    use jni::objects::{JObject, JValue};

    /// Show a share sheet with JNI context.
    ///
    /// # Errors
    /// Returns error if JNI operations fail.
    pub fn share_with_context(
        env: &mut JNIEnv,
        context: &JObject,
        sheet: &ShareSheet,
    ) -> Result<ShareResult, ShareError> {
        let intent_class = env
            .find_class("android/content/Intent")
            .map_err(|e| ShareError::Platform(format!("find_class: {e}")))?;

        let has_multiple_items = sheet.items.len() > 1;
        let action = if has_multiple_items {
            "android.intent.action.SEND_MULTIPLE"
        } else {
            "android.intent.action.SEND"
        };
        let action_jstr = env
            .new_string(action)
            .map_err(|e| ShareError::Platform(format!("new_string: {e}")))?;
        let intent = env
            .new_object(intent_class, "()V", &[])
            .map_err(|e| ShareError::Platform(format!("new_object: {e}")))?;
        env.call_method(
            &intent,
            "setAction",
            "(Ljava/lang/String;)Landroid/content/Intent;",
            &[JValue::Object(&action_jstr)],
        )
        .map_err(|e| ShareError::Platform(format!("setAction: {e}")))?;

        for item in &sheet.items {
            match item {
                ShareItem::Text(text) | ShareItem::Url(text) => {
                    let text_jstr = env
                        .new_string(text)
                        .map_err(|e| ShareError::Platform(format!("new_string: {e}")))?;
                    let extra_text = env
                        .new_string("android.intent.extra.TEXT")
                        .map_err(|e| ShareError::Platform(format!("new_string: {e}")))?;
                    env.call_method(
                        &intent,
                        "putExtra",
                        "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;",
                        &[JValue::Object(&extra_text), JValue::Object(&text_jstr)],
                    )
                    .map_err(|e| ShareError::Platform(format!("putExtra: {e}")))?;
                    let mime = env
                        .new_string("text/plain")
                        .map_err(|e| ShareError::Platform(format!("new_string: {e}")))?;
                    env.call_method(
                        &intent,
                        "setType",
                        "(Ljava/lang/String;)Landroid/content/Intent;",
                        &[JValue::Object(&mime)],
                    )
                    .map_err(|e| ShareError::Platform(format!("setType: {e}")))?;
                }
                ShareItem::Image(_) | ShareItem::File(_) => {
                    let mime = env
                        .new_string("*/*")
                        .map_err(|e| ShareError::Platform(format!("new_string: {e}")))?;
                    env.call_method(
                        &intent,
                        "setType",
                        "(Ljava/lang/String;)Landroid/content/Intent;",
                        &[JValue::Object(&mime)],
                    )
                    .map_err(|e| ShareError::Platform(format!("setType: {e}")))?;
                }
            }
        }

        if let Some(subject) = &sheet.subject {
            let subject_jstr = env
                .new_string(subject)
                .map_err(|e| ShareError::Platform(format!("new_string: {e}")))?;
            let extra_subject = env
                .new_string("android.intent.extra.SUBJECT")
                .map_err(|e| ShareError::Platform(format!("new_string: {e}")))?;
            env.call_method(
                &intent,
                "putExtra",
                "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;",
                &[
                    JValue::Object(&extra_subject),
                    JValue::Object(&subject_jstr),
                ],
            )
            .map_err(|e| ShareError::Platform(format!("putExtra: {e}")))?;
        }

        let chooser_title = env
            .new_string("Share")
            .map_err(|e| ShareError::Platform(format!("new_string: {e}")))?;
        let chooser_class = env
            .find_class("android/content/Intent")
            .map_err(|e| ShareError::Platform(format!("find_class: {e}")))?;
        let chooser = env
            .call_static_method(
                chooser_class,
                "createChooser",
                "(Landroid/content/Intent;Ljava/lang/CharSequence;)Landroid/content/Intent;",
                &[JValue::Object(&intent), JValue::Object(&chooser_title)],
            )
            .and_then(jni::objects::JValueGen::l)
            .map_err(|e| ShareError::Platform(format!("createChooser: {e}")))?;

        env.call_method(
            context,
            "startActivity",
            "(Landroid/content/Intent;)V",
            &[JValue::Object(&chooser)],
        )
        .map_err(|e| ShareError::Platform(format!("startActivity: {e}")))?;

        Ok(ShareResult::Shared)
    }
}
