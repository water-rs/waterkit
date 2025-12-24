//! Cross-platform local notifications.
//!
//! This crate provides a unified API for sending local notifications
//! across iOS, macOS, Android, Windows, and Linux platforms.

mod sys;

/// A builder for local notifications.
#[derive(Debug, Clone, Default)]
pub struct Notification {
    title: String,
    body: String,
}

impl Notification {
    /// Create a new notification builder.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            title: String::new(),
            body: String::new(),
        }
    }

    /// Set the title of the notification.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Set the body text of the notification.
    #[must_use]
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }

    /// Show the notification.
    pub fn show(self) {
        #[cfg(any(
            target_os = "linux",
            target_os = "windows",
            target_os = "macos",
            target_os = "android",
            target_os = "ios"
        ))]
        sys::show_notification(&self.title, &self.body);
    }

    /// Show the notification with an Android context.
    ///
    /// # Errors
    /// Returns an error if the notification cannot be shown.
    #[cfg(target_os = "android")]
    pub fn show_with_context(
        self,
        env: &mut jni::JNIEnv,
        context: &jni::objects::JObject,
    ) -> Result<(), String> {
        sys::android::show_notification_with_context(env, context, &self.title, &self.body)
    }
}
