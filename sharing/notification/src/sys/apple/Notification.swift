import UserNotifications
import Foundation
import os

#if os(iOS)
import UIKit
#elseif os(macOS)
import AppKit
#endif

// Simple JSON parsing for URL actions
struct NotificationAction: Codable {
    let label: String
    let url: String
}

// JSON parsing for text input actions
struct TextInputActionDef: Codable {
    let id: String
    let label: String
    let placeholder: String
    let submitLabel: String
}

// Delegate to handle notification actions
class NotificationDelegate: NSObject, UNUserNotificationCenterDelegate {
    static let shared = NotificationDelegate()

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        let actionId = response.actionIdentifier

        // Handle text input response
        if response is UNTextInputNotificationResponse {
            Logger(subsystem: "dev.waterui", category: "notification")
                .debug("Text input notification action received")
            completionHandler()
            return
        }

        // actionId is the URL to open for URL actions
        if actionId != UNNotificationDefaultActionIdentifier &&
           actionId != UNNotificationDismissActionIdentifier {
            if let url = URL(string: actionId) {
                #if os(iOS)
                DispatchQueue.main.async {
                    UIApplication.shared.open(url)
                }
                #elseif os(macOS)
                NSWorkspace.shared.open(url)
                #endif
            }
        }

        completionHandler()
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        // Show notification even when app is in foreground
        if #available(iOS 14.0, macOS 11.0, *) {
            completionHandler([.banner, .sound])
        } else {
            completionHandler([.alert, .sound])
        }
    }
}

// MARK: - Async helpers using continuations

func requestAuthorizationAsync(center: UNUserNotificationCenter) async -> Bool {
    await withCheckedContinuation { continuation in
        center.requestAuthorization(options: [.alert, .sound]) { granted, error in
            continuation.resume(returning: granted && error == nil)
        }
    }
}

func addNotificationAsync(center: UNUserNotificationCenter, request: UNNotificationRequest) async -> Bool {
    await withCheckedContinuation { continuation in
        center.add(request) { error in
            continuation.resume(returning: error == nil)
        }
    }
}

// MARK: - Main async implementation

func showNotificationAsync(
    id: String,
    title: String,
    body: String,
    subtitle: String,
    interruptionLevel: UInt8,
    actionsJson: String,
    textInputActionsJson: String
) async -> Bool {
    // On macOS, UNUserNotificationCenter requires a valid app bundle
    #if os(macOS)
    guard Bundle.main.bundleIdentifier != nil else {
        return false
    }
    #endif

    let center = UNUserNotificationCenter.current()

    // Set delegate for handling action responses
    center.delegate = NotificationDelegate.shared

    // Request authorization
    guard await requestAuthorizationAsync(center: center) else {
        return false
    }

    // Build notification content
    let content = UNMutableNotificationContent()
    content.title = title
    content.body = body

    if !subtitle.isEmpty {
        content.subtitle = subtitle
    }

    content.sound = UNNotificationSound.default

    // Set interruption level (iOS 15+, macOS 12+)
    if #available(iOS 15.0, macOS 12.0, *) {
        switch interruptionLevel {
        case 0:
            content.interruptionLevel = .passive
        case 1:
            content.interruptionLevel = .active
        case 2:
            content.interruptionLevel = .timeSensitive
        case 3:
            content.interruptionLevel = .critical
        default:
            content.interruptionLevel = .active
        }
    }

    // Collect all actions (URL actions + text input actions)
    var allActions: [UNNotificationAction] = []

    // Parse URL actions
    if !actionsJson.isEmpty,
       let data = actionsJson.data(using: .utf8),
       let actions = try? JSONDecoder().decode([NotificationAction].self, from: data) {
        for action in actions {
            allActions.append(UNNotificationAction(
                identifier: action.url,
                title: action.label,
                options: [.foreground]
            ))
        }
    }

    // Parse text input actions
    if !textInputActionsJson.isEmpty,
       let data = textInputActionsJson.data(using: .utf8),
       let textActions = try? JSONDecoder().decode([TextInputActionDef].self, from: data) {
        for action in textActions {
            allActions.append(UNTextInputNotificationAction(
                identifier: action.id,
                title: action.label,
                options: [.foreground],
                textInputButtonTitle: action.submitLabel,
                textInputPlaceholder: action.placeholder
            ))
        }
    }

    // Register category if we have any actions
    if !allActions.isEmpty {
        // Use notification ID as category base for consistency
        let categoryId = "waterkit_\(id)"
        let category = UNNotificationCategory(
            identifier: categoryId,
            actions: allActions,
            intentIdentifiers: [],
            options: []
        )

        // Register the category
        center.setNotificationCategories([category])
        content.categoryIdentifier = categoryId
    }

    // Use the provided ID as the request identifier
    // Using the same ID will replace an existing notification
    let request = UNNotificationRequest(
        identifier: id,
        content: content,
        trigger: nil
    )

    return await addNotificationAsync(center: center, request: request)
}

// MARK: - FFI entry point (sync wrapper for async code)

public func show_notification_swift(
    id: RustStr,
    title: RustStr,
    body: RustStr,
    subtitle: RustStr,
    interruption_level: UInt8,
    actions_json: RustStr,
    text_input_actions_json: RustStr
) -> Bool {
    let idStr = id.toString()
    let titleStr = title.toString()
    let bodyStr = body.toString()
    let subtitleStr = subtitle.toString()
    let actionsStr = actions_json.toString()
    let textInputActionsStr = text_input_actions_json.toString()

    // Use a semaphore to bridge sync FFI to async Swift
    let semaphore = DispatchSemaphore(value: 0)
    var result = false

    Task {
        result = await showNotificationAsync(
            id: idStr,
            title: titleStr,
            body: bodyStr,
            subtitle: subtitleStr,
            interruptionLevel: interruption_level,
            actionsJson: actionsStr,
            textInputActionsJson: textInputActionsStr
        )
        semaphore.signal()
    }

    _ = semaphore.wait(timeout: .now() + 5.0)
    return result
}
