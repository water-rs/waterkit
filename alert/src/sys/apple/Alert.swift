import Foundation
import UIKit

func show_alert_bridge(title: RustString, message: RustString, type_str: RustString, cb_id: UInt64) {
    let titleStr = title.toString()
    let messageStr = message.toString()
    
    DispatchQueue.main.async {
        guard let topVC = getTopViewController() else {
            on_alert_result(cb_id, false)
            return
        }
        
        let alert = UIAlertController(title: titleStr, message: messageStr, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "OK", style: .default) { _ in
            on_alert_result(cb_id, true)
        })
        topVC.present(alert, animated: true)
    }
}

func show_confirm_bridge(title: RustString, message: RustString, type_str: RustString, cb_id: UInt64) {
    let titleStr = title.toString()
    let messageStr = message.toString()
    
    DispatchQueue.main.async {
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
    }
}

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
