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

/// `WinRT` toasts display for about 7s (`duration="short"`) or about 25s
/// (`duration="long"`); nothing in between or beyond can be requested.
const SHORT_DURATION_MS: u32 = 7_000;
const LONG_DURATION_MS: u32 = 25_000;

/// Show a notification through `Windows.UI.Notifications`.
// The per-platform sys contract is async; WinRT toasts post synchronously.
#[allow(clippy::unused_async)]
pub async fn show_notification(
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
///
/// Anything the platform cannot render is rejected with
/// [`NotificationError::Unsupported`] instead of being silently dropped.
fn toast_document(notification: &Notification) -> Result<XmlDocument, NotificationError> {
    if notification.title.is_empty() && notification.body.is_empty() {
        return Err(NotificationError::Unsupported(
            "an empty toast needs a title or a body".into(),
        ));
    }
    // Freedesktop theme icon names have no WinRT equivalent; only file icons
    // can be shown, as the app-logo image.
    if let Some(Icon::Theme(name)) = &notification.icon {
        return Err(NotificationError::Unsupported(format!(
            "Icon::Theme({name:?}) is a freedesktop concept with no WinRT equivalent"
        )));
    }
    match &notification.sound {
        // Freedesktop sound themes and custom files have no
        // `ms-winsoundevent` equivalent.
        Some(Sound::Theme(name)) => {
            return Err(NotificationError::Unsupported(format!(
                "Sound::Theme({name:?}) has no ms-winsoundevent equivalent"
            )));
        }
        Some(Sound::File(path)) => {
            return Err(NotificationError::Unsupported(format!(
                "Sound::File({}) cannot be played by toast audio",
                path.display()
            )));
        }
        _ => {}
    }
    if notification.actions.len() > MAX_ACTIONS {
        return Err(NotificationError::Unsupported(format!(
            "{} actions; the toast schema allows at most {MAX_ACTIONS}",
            notification.actions.len()
        )));
    }
    if !notification.text_input_actions.is_empty() {
        return Err(NotificationError::Unsupported(
            "text input actions cannot be shown on Windows".into(),
        ));
    }
    // The reminder scenario keeps the toast on screen until dismissed, but
    // requires at least one action button.
    if matches!(notification.timeout, Timeout::Never) && notification.actions.is_empty() {
        return Err(NotificationError::Unsupported(
            "Timeout::Never needs an action: a reminder toast requires at least one button".into(),
        ));
    }

    let document =
        XmlDocument::new().map_err(|error| NotificationError::Platform(error.to_string()))?;

    let toast = element(&document, "toast")?;
    document
        .AppendChild(&toast)
        .map_err(|error| NotificationError::Platform(error.to_string()))?;

    // A requested duration is rounded up to the next representable one so a
    // toast never disappears earlier than asked.
    let duration = match notification.timeout {
        Timeout::Default => "short",
        Timeout::Never => "long",
        Timeout::Milliseconds(ms) if ms <= SHORT_DURATION_MS => "short",
        Timeout::Milliseconds(ms) if ms <= LONG_DURATION_MS => "long",
        Timeout::Milliseconds(ms) => {
            return Err(NotificationError::Unsupported(format!(
                "a {ms} ms toast; Windows only displays ~{SHORT_DURATION_MS} ms or ~{LONG_DURATION_MS} ms"
            )));
        }
    };
    attribute(&toast, "duration", duration)?;
    if matches!(notification.timeout, Timeout::Never) {
        attribute(&toast, "scenario", "reminder")?;
    }

    let visual = append(&document, &toast, "visual")?;
    let binding = append(&document, &visual, "binding")?;
    attribute(&binding, "template", "ToastGeneric")?;

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

    if matches!(notification.sound, Some(Sound::Suppress)) {
        let audio = append(&document, &toast, "audio")?;
        attribute(&audio, "silent", "true")?;
    }

    if !notification.actions.is_empty() {
        let actions = append(&document, &toast, "actions")?;
        for action in &notification.actions {
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
    use crate::{Action, Sound, TextInputAction, Timeout};
    use std::path::PathBuf;

    fn toast_xml(notification: &Notification) -> String {
        toast_document(notification)
            .expect("toast document")
            .GetXml()
            .expect("xml")
            .to_string()
    }

    /// The unsupported-input message when building the toast fails.
    fn unsupported(notification: &Notification) -> String {
        match toast_document(notification) {
            Err(NotificationError::Unsupported(message)) => message,
            Err(error) => panic!("expected an unsupported input, got {error:?}"),
            Ok(_) => panic!("expected an unsupported input error"),
        }
    }

    #[test]
    fn builds_escaping_title_and_body() {
        let notification = Notification::new().title("A & <B>").body("line \"two\"");
        let xml = toast_xml(&notification);
        assert!(xml.contains("A &amp; &lt;B&gt;"));
        assert!(xml.contains("line \"two\""));
    }

    #[test]
    fn rejects_empty_notification() {
        let message = unsupported(&Notification::new());
        assert!(message.contains("title"), "{message}");
    }

    #[test]
    fn maps_timeout_to_duration() {
        let cases = [
            (Timeout::Default, "short"),
            (Timeout::Milliseconds(5_000), "short"),
            (Timeout::Milliseconds(10_000), "long"),
            (Timeout::Milliseconds(25_000), "long"),
        ];
        for (timeout, duration) in cases {
            let notification = Notification::new().title("t").timeout(timeout);
            let xml = toast_xml(&notification);
            assert!(xml.contains(&format!("duration=\"{duration}\"")), "{xml}");
        }
    }

    #[test]
    fn rejects_unrepresentable_duration() {
        let notification = Notification::new()
            .title("t")
            .timeout(Timeout::Milliseconds(30_000));
        assert!(unsupported(&notification).contains("30000"));
    }

    #[test]
    fn never_timeout_uses_reminder_scenario() {
        let notification = Notification::new()
            .title("t")
            .timeout(Timeout::Never)
            .action(Action::new("View", "https://waterui.dev"));
        let xml = toast_xml(&notification);
        assert!(xml.contains("scenario=\"reminder\""), "{xml}");
    }

    #[test]
    fn never_timeout_needs_an_action() {
        let notification = Notification::new().title("t").timeout(Timeout::Never);
        assert!(unsupported(&notification).contains("Timeout::Never"));
    }

    #[test]
    fn suppresses_sound() {
        let notification = Notification::new().title("t").sound(Sound::Suppress);
        assert!(toast_xml(&notification).contains("<audio silent=\"true\"/>"));
    }

    #[test]
    fn adds_protocol_actions() {
        let notification = Notification::new()
            .title("t")
            .action(Action::new("View", "https://waterui.dev"))
            .action(Action::new("Later", "waterui://later"));
        let xml = toast_xml(&notification);
        assert!(xml.contains("activationType=\"protocol\""));
        assert!(xml.contains("content=\"View\""));
        assert!(xml.contains("arguments=\"waterui://later\""));
    }

    #[test]
    fn rejects_actions_beyond_the_schema_maximum() {
        let mut notification = Notification::new().title("t");
        for index in 0..6 {
            notification = notification.action(Action::new(format!("a{index}"), "https://x"));
        }
        assert!(unsupported(&notification).contains("6 actions"));
    }

    #[test]
    fn rejects_text_input_actions() {
        let notification = Notification::new()
            .title("t")
            .text_input_action(TextInputAction::new("reply", "Reply"));
        assert!(unsupported(&notification).contains("text input"));
    }

    #[test]
    fn maps_app_name_to_attribution() {
        let notification = Notification::new().title("t").app_name("WaterKit");
        assert!(toast_xml(&notification).contains("placement=\"attribution\""));
    }

    #[test]
    fn maps_file_icon_to_app_logo_override() {
        let notification = Notification::new()
            .title("t")
            .icon(Icon::File(std::env::current_exe().unwrap()));
        let xml = toast_xml(&notification);
        assert!(xml.contains("placement=\"appLogoOverride\""));
        assert!(xml.contains("src=\"file:///"));
    }

    #[test]
    fn rejects_theme_icon_and_custom_sounds() {
        let message = unsupported(
            &Notification::new()
                .title("t")
                .icon(Icon::Theme("mail-message-new".into())),
        );
        assert!(message.contains("Icon::Theme"), "{message}");

        let message = unsupported(
            &Notification::new()
                .title("t")
                .sound(Sound::Theme("message-new-instant".into())),
        );
        assert!(message.contains("Sound::Theme"), "{message}");

        let message = unsupported(
            &Notification::new()
                .title("t")
                .sound(Sound::File(PathBuf::from("ding.wav"))),
        );
        assert!(message.contains("Sound::File"), "{message}");
    }
}
