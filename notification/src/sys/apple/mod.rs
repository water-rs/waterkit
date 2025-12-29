//! iOS notification implementation using `UserNotifications` framework.

use crate::{Notification, NotificationError};

#[swift_bridge::bridge]
mod ffi {
    extern "Swift" {
        fn show_notification_swift(
            id: &str,
            title: &str,
            body: &str,
            subtitle: &str,
            interruption_level: u8,
            actions_json: &str,
            text_input_actions_json: &str,
        ) -> bool;
    }
}

/// Handle to a shown notification (iOS).
#[derive(Debug)]
pub struct NotificationHandleInner;

/// Show a notification using `UserNotifications`.
pub fn show_notification(
    notification: &Notification,
) -> Result<NotificationHandleInner, NotificationError> {
    let id = notification.id.as_deref().unwrap_or("");
    let subtitle = notification.subtitle.as_deref().unwrap_or("");

    // Map InterruptionLevel to iOS UNNotificationInterruptionLevel values
    let interruption_level = match notification.interruption_level {
        crate::InterruptionLevel::Passive => 0,       // .passive
        crate::InterruptionLevel::Active => 1,        // .active
        crate::InterruptionLevel::TimeSensitive => 2, // .timeSensitive
        crate::InterruptionLevel::Critical => 3,      // .critical
    };

    // Serialize actions as JSON: [{"label": "...", "url": "..."}, ...]
    let actions_json = if notification.actions.is_empty() {
        String::new()
    } else {
        let actions: Vec<String> = notification
            .actions
            .iter()
            .map(|a| format!(r#"{{"label":"{}","url":"{}"}}"#, a.label, a.url))
            .collect();
        format!("[{}]", actions.join(","))
    };

    // Serialize text input actions as JSON
    let text_input_actions_json = if notification.text_input_actions.is_empty() {
        String::new()
    } else {
        let actions: Vec<String> = notification
            .text_input_actions
            .iter()
            .map(|a| {
                format!(
                    r#"{{"id":"{}","label":"{}","placeholder":"{}","submitLabel":"{}"}}"#,
                    a.id, a.label, a.placeholder, a.submit_label
                )
            })
            .collect();
        format!("[{}]", actions.join(","))
    };

    let success = ffi::show_notification_swift(
        id,
        &notification.title,
        &notification.body,
        subtitle,
        interruption_level,
        &actions_json,
        &text_input_actions_json,
    );

    if success {
        Ok(NotificationHandleInner)
    } else {
        Err(NotificationError::Platform(
            "failed to show notification".into(),
        ))
    }
}
