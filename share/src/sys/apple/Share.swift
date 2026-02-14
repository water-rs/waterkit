import Foundation
#if os(iOS)
import UIKit
#elseif os(macOS)
import AppKit
#endif

func share_show_sheet(items_json: RustStr, subject: Optional<RustString>, callback: __private__RustFnOnceCallbackBoolNoRet) {
    let itemsStr = items_json.toString()
    let subjectStr = subject?.toString()

    var activityItems: [Any] = []
    for line in itemsStr.split(separator: "\n") {
        let lineStr = String(line)
        if lineStr.hasPrefix("text:") {
            activityItems.append(String(lineStr.dropFirst(5)))
        } else if lineStr.hasPrefix("url:") {
            if let url = URL(string: String(lineStr.dropFirst(4))) {
                activityItems.append(url)
            }
        } else if lineStr.hasPrefix("image:") {
            let path = String(lineStr.dropFirst(6))
            #if os(iOS)
            if let image = UIImage(contentsOfFile: path) {
                activityItems.append(image)
            }
            #elseif os(macOS)
            if let image = NSImage(contentsOfFile: path) {
                activityItems.append(image)
            }
            #endif
        } else if lineStr.hasPrefix("file:") {
            let path = String(lineStr.dropFirst(5))
            activityItems.append(URL(fileURLWithPath: path))
        }
    }

    DispatchQueue.main.async {
        #if os(iOS)
        let vc = UIActivityViewController(activityItems: activityItems, applicationActivities: nil)
        if let subject = subjectStr {
            vc.setValue(subject, forKey: "subject")
        }
        vc.completionWithItemsHandler = { _, completed, _, _ in
            callback.call(completed)
        }
        guard let topVC = getTopViewController() else {
            callback.call(false)
            return
        }
        topVC.present(vc, animated: true)
        #elseif os(macOS)
        let picker = NSSharingServicePicker(items: activityItems)
        // On macOS, present near the mouse location or a default view
        if let window = NSApplication.shared.mainWindow,
           let contentView = window.contentView {
            picker.show(relativeTo: .zero, of: contentView, preferredEdge: .minY)
        }
        callback.call(true)
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
