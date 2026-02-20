import Foundation
#if os(iOS)
import UIKit
#elseif os(macOS)
import AppKit
#endif

private var linkContext: UnsafeMutableRawPointer? = nil
private var initialUrl: String? = nil

func deeplink_open_url(url: RustStr, callback: __private__RustFnOnceCallbackBoolNoRet) {
    let urlStr = url.toString()
    guard let nsUrl = URL(string: urlStr) else {
        callback.call(false)
        return
    }
    #if os(iOS)
    DispatchQueue.main.async {
        UIApplication.shared.open(nsUrl, options: [:]) { success in
            callback.call(success)
        }
    }
    #elseif os(macOS)
    let success = NSWorkspace.shared.open(nsUrl)
    callback.call(success)
    #endif
}

func deeplink_can_open_url(url: RustStr) -> Bool {
    let urlStr = url.toString()
    guard let nsUrl = URL(string: urlStr) else { return false }
    #if os(iOS)
    return UIApplication.shared.canOpenURL(nsUrl)
    #elseif os(macOS)
    return true
    #endif
}

func deeplink_start_listener(link_ctx: UnsafeMutableRawPointer) {
    linkContext = link_ctx
    #if os(iOS)
    NotificationCenter.default.addObserver(
        forName: UIScene.willConnectNotification,
        object: nil,
        queue: .main
    ) { notification in
        if let scene = notification.object as? UIScene,
           let urlContexts = (scene as? UIWindowScene)?.session.configuration.storyboard {
            // URL contexts handled via scene delegate
        }
    }
    #elseif os(macOS)
    NSAppleEventManager.shared().setEventHandler(
        DeepLinkEventHandler.shared,
        andSelector: #selector(DeepLinkEventHandler.handleGetURL(event:reply:)),
        forEventClass: AEEventClass(kInternetEventClass),
        andEventID: AEEventID(kAEGetURL)
    )
    #endif
}

func deeplink_stop_listener() {
    linkContext = nil
    #if os(macOS)
    NSAppleEventManager.shared().removeEventHandler(
        forEventClass: AEEventClass(kInternetEventClass),
        andEventID: AEEventID(kAEGetURL)
    )
    #endif
}

func deeplink_get_initial_link() -> Optional<RustString> {
    if let url = initialUrl {
        return RustString(url)
    }
    return nil
}

#if os(macOS)
class DeepLinkEventHandler: NSObject {
    static let shared = DeepLinkEventHandler()

    @objc func handleGetURL(event: NSAppleEventDescriptor, reply: NSAppleEventDescriptor) {
        guard let urlString = event.paramDescriptor(forKeyword: keyDirectObject)?.stringValue else { return }
        if let ctx = linkContext {
            on_deeplink_received_raw(ctx, urlString)
        } else {
            initialUrl = urlString
        }
    }
}
#endif
