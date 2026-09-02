use crate::{ShareError, ShareResult, ShareSheet};
use futures::future;
use waterkit_build::{AndroidError, with_android_context};

impl From<AndroidError> for ShareError {
    fn from(error: AndroidError) -> Self {
        Self::Platform(error.to_string())
    }
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
    use jni::objects::{JObject, JValue};
    use jni::{Env, jni_sig, jni_str};

    /// Show a share sheet with JNI context.
    ///
    /// # Errors
    /// Returns error if JNI operations fail.
    pub fn share_with_context(
        env: &mut Env<'_>,
        context: &JObject<'_>,
        sheet: &ShareSheet,
    ) -> Result<ShareResult, ShareError> {
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
            .new_object(jni_str!("android/content/Intent"), jni_sig!("()V"), &[])
            .map_err(|e| ShareError::Platform(format!("new_object: {e}")))?;
        env.call_method(
            &intent,
            jni_str!("setAction"),
            jni_sig!("(Ljava/lang/String;)Landroid/content/Intent;"),
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
                        jni_str!("putExtra"),
                        jni_sig!("(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;"),
                        &[JValue::Object(&extra_text), JValue::Object(&text_jstr)],
                    )
                    .map_err(|e| ShareError::Platform(format!("putExtra: {e}")))?;
                    let mime = env
                        .new_string("text/plain")
                        .map_err(|e| ShareError::Platform(format!("new_string: {e}")))?;
                    env.call_method(
                        &intent,
                        jni_str!("setType"),
                        jni_sig!("(Ljava/lang/String;)Landroid/content/Intent;"),
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
                        jni_str!("setType"),
                        jni_sig!("(Ljava/lang/String;)Landroid/content/Intent;"),
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
                jni_str!("putExtra"),
                jni_sig!("(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;"),
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
        let chooser = env
            .call_static_method(
                jni_str!("android/content/Intent"),
                jni_str!("createChooser"),
                jni_sig!(
                    "(Landroid/content/Intent;Ljava/lang/CharSequence;)Landroid/content/Intent;"
                ),
                &[JValue::Object(&intent), JValue::Object(&chooser_title)],
            )
            .and_then(jni::objects::JValueOwned::l)
            .map_err(|e| ShareError::Platform(format!("createChooser: {e}")))?;

        env.call_method(
            context,
            jni_str!("startActivity"),
            jni_sig!("(Landroid/content/Intent;)V"),
            &[JValue::Object(&chooser)],
        )
        .map_err(|e| ShareError::Platform(format!("startActivity: {e}")))?;

        Ok(ShareResult::Shared)
    }
}
