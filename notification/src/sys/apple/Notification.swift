import UserNotifications
import Foundation

public func show_notification(title: String, body: String) {
    let center = UNUserNotificationCenter.current()
    center.requestAuthorization(options: [.alert, .sound]) { granted, error in
        // Ideally we handle error or rejection, but for local notification fire-and-forget:
        if granted {
            let content = UNMutableNotificationContent()
            content.title = title
            content.body = body
            content.sound = UNNotificationSound.default

            // Helper to run on main thread if needed? add() is thread safe.
            let request = UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil) // nil trigger = immediate
            center.add(request)
        }
    }
}
