//! Desktop notification implementation using notify-rust.

use notify_rust::{Notification as NrNotification, Timeout as NrTimeout};

#[cfg(target_os = "linux")]
use notify_rust::{Hint, Urgency};

use crate::{Icon, Notification, NotificationError, Timeout};

#[cfg(target_os = "linux")]
use crate::{InterruptionLevel, Sound};

/// Handle to a shown notification (platform-specific inner type).
#[derive(Debug)]
pub struct NotificationHandleInner;

/// Global map to track notification IDs to D-Bus IDs for updates (Linux only).
/// Key: our string ID, Value: D-Bus numeric ID returned by the notification server.
#[cfg(target_os = "linux")]
mod id_tracker {
    use std::collections::HashMap;
    use std::sync::Mutex;

    static NOTIFICATION_ID_MAP: Mutex<Option<HashMap<String, u32>>> = Mutex::new(None);

    pub fn get(id: &str) -> Option<u32> {
        NOTIFICATION_ID_MAP
            .lock()
            .ok()
            .and_then(|map| map.as_ref().and_then(|m| m.get(id).copied()))
    }

    pub fn set(id: &str, dbus_id: u32) {
        if let Ok(mut guard) = NOTIFICATION_ID_MAP.lock() {
            let map = guard.get_or_insert_with(HashMap::new);
            map.insert(id.to_string(), dbus_id);
        }
    }
}

/// Show a notification using notify-rust.
pub fn show_notification(
    notification: &Notification,
) -> Result<NotificationHandleInner, NotificationError> {
    let mut n = NrNotification::new();

    n.summary(&notification.title).body(&notification.body);

    // Set ID for notification replacement/update (Linux only - D-Bus supports this)
    #[cfg(target_os = "linux")]
    if let Some(ref id) = notification.id {
        if let Some(dbus_id) = id_tracker::get(id) {
            n.id(dbus_id);
        }
    }

    if let Some(ref app) = notification.app_name {
        n.appname(app);
    }

    if let Some(ref icon) = notification.icon {
        match icon {
            Icon::Theme(name) => {
                n.icon(name);
            }
            Icon::File(path) => {
                n.icon(&format!("file://{}", path.display()));
            }
        }
    }

    // Linux-only features: urgency, hints, actions
    #[cfg(target_os = "linux")]
    {
        // InterruptionLevel → notify-rust Urgency
        n.urgency(match notification.interruption_level {
            InterruptionLevel::Passive => Urgency::Low,
            InterruptionLevel::Active => Urgency::Normal,
            InterruptionLevel::TimeSensitive | InterruptionLevel::Critical => Urgency::Critical,
        });

        // Sound
        if let Some(ref sound) = notification.sound {
            match sound {
                Sound::Default => {}
                Sound::Theme(name) => {
                    n.hint(Hint::SoundName(name.clone()));
                }
                Sound::File(path) => {
                    n.hint(Hint::SoundFile(path.display().to_string()));
                }
                Sound::Suppress => {
                    n.hint(Hint::SuppressSound(true));
                }
            }
        }

        // Actions - use URL as action ID so we can open it on click
        for action in &notification.actions {
            n.action(&action.url, &action.label);
        }
    }

    // Timeout (supported on all platforms via notify-rust)
    n.timeout(match notification.timeout {
        Timeout::Default => NrTimeout::Default,
        Timeout::Never => NrTimeout::Never,
        Timeout::Milliseconds(ms) => NrTimeout::Milliseconds(ms),
    });

    // Subtitle (macOS only)
    #[cfg(target_os = "macos")]
    if let Some(ref subtitle) = notification.subtitle {
        n.subtitle(subtitle);
    }

    #[cfg(target_os = "linux")]
    {
        let handle = n
            .show()
            .map_err(|e| NotificationError::Platform(e.to_string()))?;

        // Store the D-Bus ID for future updates
        if let Some(ref id) = notification.id {
            id_tracker::set(id, handle.id());
        }

        // If there are actions, spawn a handler thread to open URLs on click
        if !notification.actions.is_empty() {
            std::thread::spawn(move || {
                handle.wait_for_action(|action_url| {
                    // The action ID is the URL to open
                    if !action_url.is_empty() && action_url != "__closed" {
                        let _ = open::that(action_url);
                    }
                });
            });
        }

        return Ok(NotificationHandleInner);
    }

    #[cfg(not(target_os = "linux"))]
    {
        n.show()
            .map_err(|e| NotificationError::Platform(e.to_string()))?;

        Ok(NotificationHandleInner)
    }
}
