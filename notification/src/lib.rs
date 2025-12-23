mod sys;

pub struct Notification {
    title: String,
    body: String,
}

impl Notification {
    pub fn new() -> Self {
        Self {
            title: String::new(),
            body: String::new(),
        }
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }

    pub fn show(self) {
        #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos", target_os = "android", target_os = "ios"))]
        sys::show_notification(&self.title, &self.body);
    }

    #[cfg(target_os = "android")]
    pub fn show_with_context(self, env: &mut jni::JNIEnv, context: &jni::objects::JObject) -> Result<(), String> {
        sys::android::show_notification_with_context(env, context, &self.title, &self.body)
    }
}

impl Default for Notification {
    fn default() -> Self {
        Self::new()
    }
}
