//! macOS notification implementation.
//!
//! For bundled apps: Uses `UserNotifications` framework (supports actions).
//! For CLI/unbundled apps: Uses notify-rust (no actions support).

use crate::{Notification, NotificationError};

/// Handle to a shown notification.
#[derive(Debug)]
pub struct NotificationHandleInner {
    #[allow(dead_code)]
    desktop_handle: Option<super::desktop::NotificationHandleInner>,
}

/// Check if running in a bundled macOS app.
///
/// A macOS app bundle has the structure: `Foo.app/Contents/MacOS/executable`.
/// This checks if the current executable is inside a `.app` directory with an `Info.plist`.
fn is_bundled_app() -> bool {
    let Some(exe_path) = std::env::current_exe().ok() else {
        return false;
    };

    // Navigate up: executable -> MacOS -> Contents -> Foo.app
    let Some(contents_dir) = exe_path.parent().and_then(|p| p.parent()) else {
        return false;
    };

    // Check if parent ends with .app and has Info.plist
    let Some(app_dir) = contents_dir.parent() else {
        return false;
    };

    app_dir.extension().is_some_and(|ext| ext == "app") && contents_dir.join("Info.plist").exists()
}

/// Show a notification on macOS.
pub fn show_notification(
    notification: &Notification,
) -> Result<NotificationHandleInner, NotificationError> {
    // Try native UserNotifications first (for bundled apps)
    if is_bundled_app() {
        super::apple::show_notification(notification)?;
        return Ok(NotificationHandleInner {
            desktop_handle: None,
        });
    }

    // Not a bundled app - check if actions were requested
    if !notification.actions.is_empty() {
        return Err(NotificationError::Platform(
            "notification actions require a bundled macOS app; \
             run from an .app bundle or remove actions"
                .into(),
        ));
    }

    // Use notify-rust for unbundled apps (basic notifications only)
    let handle = super::desktop::show_notification(notification)?;
    Ok(NotificationHandleInner {
        desktop_handle: Some(handle),
    })
}
