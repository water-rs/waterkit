//! Windows notification implementation through `WinRT` `Windows.UI.Notifications`.
//!
//! The toast XML is built through the `Windows.Data.Xml.Dom` DOM API so all
//! text and attribute escaping is handled by the document serializer.

use windows::Data::Xml::Dom::{XmlDocument, XmlElement};
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
use windows::core::HSTRING;

use crate::{Icon, Notification, NotificationError, Sound, Timeout};

/// Handle to a shown notification.
#[derive(Debug)]
pub struct NotificationHandleInner;

/// `AppUserModelID` handed to `CreateToastNotifierWithId`.
///
/// Showing a toast requires an AUMID registered with the shell, which only
/// packaged/installed apps have. Borrow the id of the PowerShell shortcut
/// that ships with Windows — the same workaround notify-rust used through
/// `tauri-winrt-notification`.
const APP_USER_MODEL_ID: &str =
    r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe";

/// The toast schema allows at most this many action buttons.
const MAX_ACTIONS: usize = 5;

/// Show a notification through `Windows.UI.Notifications`.
pub fn show_notification(
    notification: &Notification,
) -> Result<NotificationHandleInner, NotificationError> {
    let toast = ToastNotification::CreateToastNotification(&toast_document(notification)?)
        .map_err(|error| NotificationError::Platform(error.to_string()))?;

    // `NotificationHandle::update` re-shows with the same id; a matching tag
    // makes the new toast replace the old one. Win10 1703+ allows 64 chars;
    // generated ids are 36-char UUIDs.
    if let Some(id) = &notification.id {
        toast
            .SetTag(&HSTRING::from(id.as_str()))
            .map_err(|error| NotificationError::Platform(error.to_string()))?;
    }

    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_USER_MODEL_ID))
        .and_then(|notifier| notifier.Show(&toast))
        .map_err(|error| NotificationError::Platform(error.to_string()))?;

    Ok(NotificationHandleInner)
}

/// Build the toast payload through the `XmlDocument` DOM API.
fn toast_document(notification: &Notification) -> Result<XmlDocument, NotificationError> {
    let document =
        XmlDocument::new().map_err(|error| NotificationError::Platform(error.to_string()))?;

    let toast = element(&document, "toast")?;
    document
        .AppendChild(&toast)
        .map_err(|error| NotificationError::Platform(error.to_string()))?;

    // WinRT only supports ~7s ("short") and ~25s ("long") durations; this is
    // the same mapping notify-rust used.
    let duration = match notification.timeout {
        Timeout::Default => "short",
        Timeout::Never => "long",
        Timeout::Milliseconds(ms) => {
            if ms >= 25_000 {
                "long"
            } else {
                "short"
            }
        }
    };
    attribute(&toast, "duration", duration)?;

    let visual = append(&document, &toast, "visual")?;
    let binding = append(&document, &visual, "binding")?;
    attribute(&binding, "template", "ToastGeneric")?;

    // Freedesktop theme icon names have no WinRT equivalent; only file icons
    // can be shown, as the app-logo image.
    if let Some(Icon::File(path)) = &notification.icon {
        let image = append(&document, &binding, "image")?;
        attribute(&image, "placement", "appLogoOverride")?;
        let absolute = path.canonicalize().map_err(|error| {
            NotificationError::Platform(format!("icon {}: {error}", path.display()))
        })?;
        let file_url = url::Url::from_file_path(&absolute).map_err(|()| {
            NotificationError::Platform(format!("icon {}: not a file URL", absolute.display()))
        })?;
        attribute(&image, "src", file_url.as_str())?;
    }

    for line in [&notification.title, &notification.body] {
        if !line.is_empty() {
            append_text(&document, &binding, line)?;
        }
    }

    if let Some(app_name) = &notification.app_name {
        let text = append_text(&document, &binding, app_name)?;
        attribute(&text, "placement", "attribution")?;
    }

    // Freedesktop sound themes and custom files have no `ms-winsoundevent`
    // equivalent; those keep the toast's default system sound.
    if matches!(notification.sound, Some(Sound::Suppress)) {
        let audio = append(&document, &toast, "audio")?;
        attribute(&audio, "silent", "true")?;
    }

    if !notification.actions.is_empty() {
        let actions = append(&document, &toast, "actions")?;
        for action in notification.actions.iter().take(MAX_ACTIONS) {
            let element = append(&document, &actions, "action")?;
            // Protocol activation makes the shell open the URL on click,
            // which is all `Action` needs.
            attribute(&element, "activationType", "protocol")?;
            attribute(&element, "content", &action.label)?;
            attribute(&element, "arguments", &action.url)?;
        }
    }

    Ok(document)
}

fn element(document: &XmlDocument, name: &str) -> Result<XmlElement, NotificationError> {
    document
        .CreateElement(&HSTRING::from(name))
        .map_err(|error| NotificationError::Platform(error.to_string()))
}

fn append(
    document: &XmlDocument,
    parent: &XmlElement,
    name: &str,
) -> Result<XmlElement, NotificationError> {
    let child = element(document, name)?;
    parent
        .AppendChild(&child)
        .map_err(|error| NotificationError::Platform(error.to_string()))?;
    Ok(child)
}

fn append_text(
    document: &XmlDocument,
    parent: &XmlElement,
    content: &str,
) -> Result<XmlElement, NotificationError> {
    let text = append(document, parent, "text")?;
    let node = document
        .CreateTextNode(&HSTRING::from(content))
        .map_err(|error| NotificationError::Platform(error.to_string()))?;
    text.AppendChild(&node)
        .map_err(|error| NotificationError::Platform(error.to_string()))?;
    Ok(text)
}

fn attribute(element: &XmlElement, name: &str, value: &str) -> Result<(), NotificationError> {
    element
        .SetAttribute(&HSTRING::from(name), &HSTRING::from(value))
        .map_err(|error| NotificationError::Platform(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Action, Sound, Timeout};
    use std::path::PathBuf;

    fn toast_xml(notification: &Notification) -> String {
        toast_document(notification)
            .expect("toast document")
            .GetXml()
            .expect("xml")
            .to_string()
    }

    #[test]
    fn builds_escaping_title_and_body() {
        let notification = Notification::new().title("A & <B>").body("line \"two\"");
        let xml = toast_xml(&notification);
        assert!(xml.contains("A &amp; &lt;B&gt;"));
        assert!(xml.contains("line \"two\""));
    }

    #[test]
    fn maps_timeout_to_duration() {
        let cases = [
            (Timeout::Default, "short"),
            (Timeout::Never, "long"),
            (Timeout::Milliseconds(5_000), "short"),
            (Timeout::Milliseconds(30_000), "long"),
        ];
        for (timeout, duration) in cases {
            let notification = Notification::new().timeout(timeout);
            let xml = toast_xml(&notification);
            assert!(xml.contains(&format!("duration=\"{duration}\"")), "{xml}");
        }
    }

    #[test]
    fn suppresses_sound() {
        let notification = Notification::new().sound(Sound::Suppress);
        assert!(toast_xml(&notification).contains("<audio silent=\"true\"/>"));
    }

    #[test]
    fn adds_protocol_actions() {
        let notification = Notification::new()
            .action(Action::new("View", "https://waterui.dev"))
            .action(Action::new("Later", "waterui://later"));
        let xml = toast_xml(&notification);
        assert!(xml.contains("activationType=\"protocol\""));
        assert!(xml.contains("content=\"View\""));
        assert!(xml.contains("arguments=\"waterui://later\""));
    }

    #[test]
    fn caps_actions_at_schema_maximum() {
        let mut notification = Notification::new();
        for index in 0..7 {
            notification = notification.action(Action::new(format!("a{index}"), "https://x"));
        }
        let xml = toast_xml(&notification);
        assert!(xml.contains("content=\"a4\""));
        assert!(!xml.contains("content=\"a5\""));
    }

    #[test]
    fn maps_app_name_to_attribution() {
        let notification = Notification::new().app_name("WaterKit");
        assert!(toast_xml(&notification).contains("placement=\"attribution\""));
    }

    #[test]
    fn maps_file_icon_to_app_logo_override() {
        let notification = Notification::new().icon(Icon::File(std::env::current_exe().unwrap()));
        let xml = toast_xml(&notification);
        assert!(xml.contains("placement=\"appLogoOverride\""));
        assert!(xml.contains("src=\"file:///"));
    }

    #[test]
    fn ignores_theme_icon_and_custom_sound() {
        let notification = Notification::new()
            .icon(Icon::Theme("mail-message-new".into()))
            .sound(Sound::File(PathBuf::from("ding.wav")));
        let xml = toast_xml(&notification);
        assert!(!xml.contains("<image"));
        assert!(!xml.contains("<audio"));
    }
}
