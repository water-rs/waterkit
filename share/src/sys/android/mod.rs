use crate::{ShareError, ShareResult, ShareSheet};

#[allow(clippy::unused_async)]
pub async fn show_share_sheet(_sheet: ShareSheet) -> Result<ShareResult, ShareError> {
    Err(ShareError::PlatformError(
        "Android: use share_with_context() with JNIEnv and Context".into(),
    ))
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
            .map_err(|e| ShareError::PlatformError(format!("find_class: {e}")))?;

        let has_multiple_items = sheet.items.len() > 1;
        let action = if has_multiple_items {
            "android.intent.action.SEND_MULTIPLE"
        } else {
            "android.intent.action.SEND"
        };
        let action_jstr = env
            .new_string(action)
            .map_err(|e| ShareError::PlatformError(format!("new_string: {e}")))?;
        let intent = env
            .new_object(intent_class, "()V", &[])
            .map_err(|e| ShareError::PlatformError(format!("new_object: {e}")))?;
        env.call_method(
            &intent,
            "setAction",
            "(Ljava/lang/String;)Landroid/content/Intent;",
            &[JValue::Object(&action_jstr)],
        )
        .map_err(|e| ShareError::PlatformError(format!("setAction: {e}")))?;

        for item in &sheet.items {
            match item {
                ShareItem::Text(text) | ShareItem::Url(text) => {
                    let text_jstr = env
                        .new_string(text)
                        .map_err(|e| ShareError::PlatformError(format!("new_string: {e}")))?;
                    let extra_text = env
                        .new_string("android.intent.extra.TEXT")
                        .map_err(|e| ShareError::PlatformError(format!("new_string: {e}")))?;
                    env.call_method(
                        &intent,
                        "putExtra",
                        "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;",
                        &[JValue::Object(&extra_text), JValue::Object(&text_jstr)],
                    )
                    .map_err(|e| ShareError::PlatformError(format!("putExtra: {e}")))?;
                    let mime = env
                        .new_string("text/plain")
                        .map_err(|e| ShareError::PlatformError(format!("new_string: {e}")))?;
                    env.call_method(
                        &intent,
                        "setType",
                        "(Ljava/lang/String;)Landroid/content/Intent;",
                        &[JValue::Object(&mime)],
                    )
                    .map_err(|e| ShareError::PlatformError(format!("setType: {e}")))?;
                }
                ShareItem::Image(_) | ShareItem::File(_) => {
                    let mime = env
                        .new_string("*/*")
                        .map_err(|e| ShareError::PlatformError(format!("new_string: {e}")))?;
                    env.call_method(
                        &intent,
                        "setType",
                        "(Ljava/lang/String;)Landroid/content/Intent;",
                        &[JValue::Object(&mime)],
                    )
                    .map_err(|e| ShareError::PlatformError(format!("setType: {e}")))?;
                }
            }
        }

        if let Some(subject) = &sheet.subject {
            let subject_jstr = env
                .new_string(subject)
                .map_err(|e| ShareError::PlatformError(format!("new_string: {e}")))?;
            let extra_subject = env
                .new_string("android.intent.extra.SUBJECT")
                .map_err(|e| ShareError::PlatformError(format!("new_string: {e}")))?;
            env.call_method(
                &intent,
                "putExtra",
                "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;",
                &[
                    JValue::Object(&extra_subject),
                    JValue::Object(&subject_jstr),
                ],
            )
            .map_err(|e| ShareError::PlatformError(format!("putExtra: {e}")))?;
        }

        let chooser_title = env
            .new_string("Share")
            .map_err(|e| ShareError::PlatformError(format!("new_string: {e}")))?;
        let chooser_class = env
            .find_class("android/content/Intent")
            .map_err(|e| ShareError::PlatformError(format!("find_class: {e}")))?;
        let chooser = env
            .call_static_method(
                chooser_class,
                "createChooser",
                "(Landroid/content/Intent;Ljava/lang/CharSequence;)Landroid/content/Intent;",
                &[JValue::Object(&intent), JValue::Object(&chooser_title)],
            )
            .and_then(jni::objects::JValueGen::l)
            .map_err(|e| ShareError::PlatformError(format!("createChooser: {e}")))?;

        env.call_method(
            context,
            "startActivity",
            "(Landroid/content/Intent;)V",
            &[JValue::Object(&chooser)],
        )
        .map_err(|e| ShareError::PlatformError(format!("startActivity: {e}")))?;

        Ok(ShareResult::Shared)
    }
}
