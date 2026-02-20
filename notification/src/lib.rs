//! Cross-platform local notifications.
//!
//! This crate provides a unified API for sending local notifications
//! across iOS, macOS, Android, Windows, and Linux platforms.
//!
//! # Example
//!
//! ```no_run
//! use waterkit_notification::{Notification, InterruptionLevel};
//!
//! fn main() -> Result<(), waterkit_notification::NotificationError> {
//!     Notification::new()
//!         .title("Hello")
//!         .body("World from WaterKit!")
//!         .show()?;
//!     Ok(())
//! }
//! ```
//!
//! # Notification Updates
//!
//! Use the handle to update an existing notification:
//!
//! ```no_run
//! use waterkit_notification::Notification;
//!
//! // Show initial notification
//! let handle = Notification::new()
//!     .title("Downloading...")
//!     .body("0%")
//!     .show()?;
//!
//! // Update the same notification using the handle
//! handle.update()
//!     .title("Downloading...")
//!     .body("50%")
//!     .show()?;
//! # Ok::<(), waterkit_notification::NotificationError>(())
//! ```
//!
//! # Platform Feature Support
//!
//! | Feature | Linux | macOS | Windows | iOS | Android |
//! |---------|-------|-------|---------|-----|---------|
//! | title/body | ✓ | ✓ | ✓ | ✓ | ✓ |
//! | icon | ✓ | ✓ | ✓ | ✗ | ✗ |
//! | subtitle | ✗ | ✓ | ✗ | ✓ | ✗ |
//! | interruption_level | ✓ | ✗ | ✗ | ✓ | ✓ |
//! | timeout | ✓ | ✗ | ✗ | ✗ | ✗ |
//! | sound | ✓ | ✗ | ✗ | ✓ | ✓ |
//! | actions (URL) | ✓ | ✓ | ✗ | ✓ | ✓ |
//! | quick reply | ✗ | ✓ | ✗ | ✓ | ✓ |
//! | update by ID | ✓ | ✓ | ✓ | ✓ | ✓ |

mod error;
mod sys;

use std::path::PathBuf;

pub use error::NotificationError;

/// Controls notification interruption behavior across platforms.
///
/// | Level | iOS | Android | Linux |
/// |-------|-----|---------|-------|
/// | Passive | Silent, no wake | `IMPORTANCE_LOW` | Low urgency |
/// | Active | Default sounds | `IMPORTANCE_DEFAULT` | Normal urgency |
/// | `TimeSensitive` | Breaks Focus mode | `IMPORTANCE_HIGH` | Critical urgency |
/// | Critical | Breaks silent switch* | `IMPORTANCE_MAX` | Critical urgency |
///
/// *iOS Critical requires Apple entitlement approval.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InterruptionLevel {
    /// Delivered silently, doesn't wake screen.
    Passive,
    /// Default behavior with sound.
    #[default]
    Active,
    /// Time-sensitive: breaks through Focus/DND modes.
    TimeSensitive,
    /// Critical: highest priority (iOS requires entitlement).
    Critical,
}

/// Timeout duration for notification display.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Timeout {
    /// Use system default timeout.
    #[default]
    Default,
    /// Never auto-dismiss (user must close manually).
    Never,
    /// Dismiss after specified milliseconds.
    Milliseconds(u32),
}

/// Notification icon source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Icon {
    /// Freedesktop.org theme icon name (e.g., "mail-message-new").
    Theme(String),
    /// Path to an image file.
    File(PathBuf),
}

/// Notification sound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sound {
    /// Play default system notification sound.
    Default,
    /// Freedesktop.org sound theme name (e.g., "message-new-instant").
    Theme(String),
    /// Path to a sound file.
    File(PathBuf),
    /// Suppress notification sound.
    Suppress,
}

/// An interactive action button for notifications.
///
/// Actions can open URLs or deep links when clicked.
///
/// # Platform Support
///
/// | Platform | URL Actions |
/// |----------|-------------|
/// | Linux | ✓ via D-Bus |
/// | macOS | ✓ via `UNNotificationAction` |
/// | Windows | ✓ via protocol activation |
/// | iOS | ✓ via `UNNotificationAction` |
/// | Android | ✓ via `PendingIntent` |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    /// Display label for the button.
    pub label: String,
    /// URL or deep link to open when clicked.
    pub url: String,
}

impl Action {
    /// Create a new URL action.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use waterkit_notification::Action;
    ///
    /// let action = Action::new("View Details", "https://waterui.dev");
    /// let deep_link = Action::new("Open Settings", "waterui://settings");
    /// ```
    #[must_use]
    pub fn new(label: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            url: url.into(),
        }
    }
}

/// A text input action for quick reply notifications.
///
/// When the user taps this action, a text input field appears allowing
/// them to type a response directly in the notification.
///
/// # Platform Support
///
/// | Platform | Support |
/// |----------|---------|
/// | iOS | ✓ via `UNTextInputNotificationAction` |
/// | macOS | ✓ via `UNTextInputNotificationAction` |
/// | Android | ✓ via `RemoteInput` |
/// | Linux | ✗ (not in D-Bus spec) |
/// | Windows | ✗ |
///
/// # Example
///
/// ```no_run
/// use waterkit_notification::TextInputAction;
///
/// let reply = TextInputAction::new("reply", "Reply")
///     .placeholder("Type a message...");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextInputAction {
    /// Unique identifier for this action.
    pub id: String,
    /// Button label (e.g., "Reply").
    pub label: String,
    /// Placeholder text in the input field.
    pub placeholder: String,
    /// Submit button label (iOS/macOS only).
    pub submit_label: String,
}

impl TextInputAction {
    /// Create a new text input action.
    ///
    /// # Arguments
    ///
    /// * `id` - Unique identifier for the action
    /// * `label` - Button label shown to the user
    #[must_use]
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            placeholder: String::new(),
            submit_label: "Send".into(),
        }
    }

    /// Set the placeholder text for the input field.
    #[must_use]
    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = text.into();
        self
    }

    /// Set the submit button label (iOS/macOS only).
    #[must_use]
    pub fn submit_label(mut self, label: impl Into<String>) -> Self {
        self.submit_label = label.into();
        self
    }
}

/// Response from a notification action.
///
/// This is passed to the callback set via [`Notification::on_response`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationResponse {
    /// User clicked a URL action.
    UrlAction {
        /// The notification ID.
        notification_id: String,
        /// The action URL.
        url: String,
    },
    /// User submitted text via quick reply.
    TextInput {
        /// The notification ID.
        notification_id: String,
        /// The action ID.
        action_id: String,
        /// The text entered by the user.
        text: String,
    },
    /// User dismissed the notification.
    Dismissed {
        /// The notification ID.
        notification_id: String,
    },
}

/// Handle to a shown notification.
///
/// Use this handle to update or replace the notification content.
///
/// # Example
///
/// ```no_run
/// use waterkit_notification::Notification;
///
/// let mut handle = Notification::new()
///     .title("Downloading...")
///     .body("0%")
///     .show()?;
///
/// // Update the notification
/// handle.update()
///     .body("50%")
///     .show()?;
/// # Ok::<(), waterkit_notification::NotificationError>(())
/// ```
#[derive(Debug)]
pub struct NotificationHandle {
    #[allow(dead_code)]
    inner: sys::NotificationHandleInner,
    id: String,
}

impl NotificationHandle {
    /// Create a new notification that will update/replace this one.
    ///
    /// The returned `Notification` has the same ID, so calling `show()`
    /// will replace the existing notification instead of creating a new one.
    #[must_use]
    pub fn update(&self) -> Notification {
        Notification {
            id: Some(self.id.clone()),
            ..Default::default()
        }
    }
}

/// A builder for local notifications.
#[derive(Debug, Clone, Default)]
pub struct Notification {
    pub(crate) id: Option<String>,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) subtitle: Option<String>,
    pub(crate) app_name: Option<String>,
    pub(crate) icon: Option<Icon>,
    pub(crate) sound: Option<Sound>,
    pub(crate) interruption_level: InterruptionLevel,
    pub(crate) timeout: Timeout,
    pub(crate) actions: Vec<Action>,
    pub(crate) text_input_actions: Vec<TextInputAction>,
}

impl Notification {
    /// Create a new notification builder.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            id: None,
            title: String::new(),
            body: String::new(),
            subtitle: None,
            app_name: None,
            icon: None,
            sound: None,
            interruption_level: InterruptionLevel::Active,
            timeout: Timeout::Default,
            actions: Vec::new(),
            text_input_actions: Vec::new(),
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

    /// Set the subtitle of the notification.
    ///
    /// **macOS/iOS only**: On other platforms, this is ignored.
    #[must_use]
    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// Set the application name for the notification.
    #[must_use]
    pub fn app_name(mut self, name: impl Into<String>) -> Self {
        self.app_name = Some(name.into());
        self
    }

    /// Set the notification icon.
    ///
    /// **Desktop only**: On mobile platforms, this is ignored.
    #[must_use]
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Set the notification sound.
    ///
    /// **Linux/iOS/Android**: Custom sounds are supported.
    /// **macOS/Windows**: Only default sound is used.
    #[must_use]
    pub fn sound(mut self, sound: Sound) -> Self {
        self.sound = Some(sound);
        self
    }

    /// Set the interruption level for the notification.
    #[must_use]
    pub const fn interruption_level(mut self, level: InterruptionLevel) -> Self {
        self.interruption_level = level;
        self
    }

    /// Convenience method to set as time-sensitive.
    ///
    /// Time-sensitive notifications break through Focus/DND modes.
    #[must_use]
    pub const fn time_sensitive(self) -> Self {
        self.interruption_level(InterruptionLevel::TimeSensitive)
    }

    /// Set the timeout duration for the notification.
    ///
    /// **Linux only**: On other platforms, this is ignored.
    #[must_use]
    pub const fn timeout(mut self, timeout: Timeout) -> Self {
        self.timeout = timeout;
        self
    }

    /// Add an action button to the notification.
    #[must_use]
    pub fn action(mut self, action: Action) -> Self {
        self.actions.push(action);
        self
    }

    /// Add a text input action (quick reply) to the notification.
    ///
    /// When the user taps this action, a text input field appears.
    ///
    /// **Supported:** iOS, macOS, Android
    /// **Not supported:** Linux, Windows (silently ignored)
    #[must_use]
    pub fn text_input_action(mut self, action: TextInputAction) -> Self {
        self.text_input_actions.push(action);
        self
    }

    /// Show the notification.
    ///
    /// # Errors
    ///
    /// Returns an error if the notification cannot be shown.
    pub fn show(mut self) -> Result<NotificationHandle, NotificationError> {
        // Generate ID if not provided
        let id = self
            .id
            .take()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        self.id = Some(id.clone());

        #[cfg(any(
            target_os = "linux",
            target_os = "windows",
            target_os = "macos",
            target_os = "android",
            target_os = "ios"
        ))]
        {
            let inner = sys::show_notification(&self)?;
            Ok(NotificationHandle { inner, id })
        }

        #[cfg(not(any(
            target_os = "linux",
            target_os = "windows",
            target_os = "macos",
            target_os = "android",
            target_os = "ios"
        )))]
        {
            let _ = id; // suppress unused warning
            Err(NotificationError::Platform(
                "notifications not supported on this platform".into(),
            ))
        }
    }

    /// Show the notification with an Android context.
    ///
    /// # Errors
    ///
    /// Returns an error if the notification cannot be shown.
    #[cfg(target_os = "android")]
    pub fn show_with_context(
        mut self,
        env: &mut jni::JNIEnv,
        context: &jni::objects::JObject,
    ) -> Result<NotificationHandle, NotificationError> {
        // Generate ID if not provided
        let id = self
            .id
            .take()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        self.id = Some(id.clone());

        let inner = sys::android::show_notification_with_context(env, context, &self)?;
        Ok(NotificationHandle { inner, id })
    }
}
