use notify_rust::Notification as NrNotification;

pub fn show_notification(title: &str, body: &str) {
    let _ = NrNotification::new()
        .summary(title)
        .body(body)
        .show();
}
