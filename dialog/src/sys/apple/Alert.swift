import Foundation
#if os(iOS)
import UIKit
#elseif os(macOS)
import AppKit
#endif

func show_alert_bridge(title: RustStr, message: RustStr, type_str: RustStr, cb_id: UInt64) {
    let titleStr = title.toString()
    let messageStr = message.toString()
    
    DispatchQueue.main.async {
        #if os(iOS)
        guard let topVC = getTopViewController() else {
            on_alert_result(cb_id, false)
            return
        }
        
        let alert = UIAlertController(title: titleStr, message: messageStr, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "OK", style: .default) { _ in
            on_alert_result(cb_id, true)
        })
        topVC.present(alert, animated: true)
        #elseif os(macOS)
        let alert = NSAlert()
        alert.messageText = titleStr
        alert.informativeText = messageStr
        alert.alertStyle = .informational // simplified mapping
        alert.addButton(withTitle: "OK")
        let _ = alert.runModal()
        on_alert_result(cb_id, true)
        #endif
    }
}

func show_confirm_bridge(title: RustStr, message: RustStr, type_str: RustStr, cb_id: UInt64) {
    let titleStr = title.toString()
    let messageStr = message.toString()
    
    DispatchQueue.main.async {
        #if os(iOS)
        guard let topVC = getTopViewController() else {
            on_alert_result(cb_id, false)
            return
        }
        
        let alert = UIAlertController(title: titleStr, message: messageStr, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "OK", style: .default) { _ in
            on_alert_result(cb_id, true)
        })
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel) { _ in
            on_alert_result(cb_id, false)
        })
        topVC.present(alert, animated: true)
        #elseif os(macOS)
        let alert = NSAlert()
        alert.messageText = titleStr
        alert.informativeText = messageStr
        alert.alertStyle = .warning // simplified
        alert.addButton(withTitle: "OK")
        alert.addButton(withTitle: "Cancel")
        let response = alert.runModal()
        on_alert_result(cb_id, response == .alertFirstButtonReturn)
        #endif
    }
}

#if os(iOS)
private func getTopViewController() -> UIViewController? {
    let keyWindow = UIApplication.shared.connectedScenes
        .filter({$0.activationState == .foregroundActive})
        .map({$0 as? UIWindowScene})
        .compactMap({$0})
        .first?.windows
        .filter({$0.isKeyWindow}).first
        
    var top = keyWindow?.rootViewController ?? UIApplication.shared.delegate?.window??.rootViewController
    
    while let presented = top?.presentedViewController {
        top = presented
    }
    return top
}
#endif
