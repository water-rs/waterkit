import Foundation
#if os(iOS)
import UIKit
import PhotosUI
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

// Keep delegates alive
private var activeDelegates: [UInt64: Any] = [:]
// Keep providers alive for handles
private var activeProviders: [UInt64: NSItemProvider] = [:]
private var nextHandleId: UInt64 = 1

func show_photo_picker_bridge(media_type: RustStr, cb_id: UInt64) {
    let typeStr = media_type.toString()
    
    DispatchQueue.main.async {
        guard let topVC = getTopViewController() else {
            on_photo_picker_result(cb_id, nil)
            return
        }
        
        var config = PHPickerConfiguration()
        config.selectionLimit = 1
        
        if typeStr == "video" {
            config.filter = .videos
        } else if typeStr == "livephoto" {
             config.filter = .livePhotos
        } else {
             config.filter = .images
        }
        
        // Setup delegate
        let delegate = PhotoPickerDelegate(cb_id: cb_id)
        activeDelegates[cb_id] = delegate
        
        let picker = PHPickerViewController(configuration: config)
        picker.delegate = delegate
        
        topVC.present(picker, animated: true)
    }
}

func load_media_bridge(handle_id: UInt64, cb_id: UInt64) {
    DispatchQueue.main.async {
        guard let provider = activeProviders[handle_id] else {
            on_load_media_result(cb_id, nil)
            return
        }
        
        // Clean up provider reference after loading? 
        // Or keep it? The "Handle" implies ownership. If we load, we might want to keep it valid (re-loadable).
        // For now, let's keep it.
        
        if provider.hasItemConformingToTypeIdentifier(UTType.movie.identifier) {
             loadFile(provider, type: UTType.movie.identifier, cb_id: cb_id)
        } else if provider.hasItemConformingToTypeIdentifier(UTType.image.identifier) {
             loadFile(provider, type: UTType.image.identifier, cb_id: cb_id)
        } else {
             on_load_media_result(cb_id, nil)
        }
    }
}

private func loadFile(_ provider: NSItemProvider, type: String, cb_id: UInt64) {
    provider.loadFileRepresentation(forTypeIdentifier: type) { url, error in
        guard let url = url else {
            on_load_media_result(cb_id, nil)
            return
        }
        
        // Copy to tmp
        let tmpDir = FileManager.default.temporaryDirectory
        let fileName = UUID().uuidString + "." + url.pathExtension
        let dstUrl = tmpDir.appendingPathComponent(fileName)
        
        do {
            if FileManager.default.fileExists(atPath: dstUrl.path) {
                try FileManager.default.removeItem(at: dstUrl)
            }
            try FileManager.default.copyItem(at: url, to: dstUrl)
            on_load_media_result(cb_id, dstUrl.path)
        } catch {
            print("Error copying file: \(error)")
            on_load_media_result(cb_id, nil)
        }
    }
}


class PhotoPickerDelegate: NSObject, PHPickerViewControllerDelegate {
    let cb_id: UInt64
    
    init(cb_id: UInt64) {
        self.cb_id = cb_id
    }
    
    func picker(_ picker: PHPickerViewController, didFinishPicking results: [PHPickerResult]) {
        picker.dismiss(animated: true) {
            // Remove self from active delegates after dismissal
            activeDelegates.removeValue(forKey: self.cb_id)
        }
        
        guard let result = results.first else {
            on_photo_picker_result(cb_id, nil)
            return
        }
        
        // Store provider and return handle
        let handleId = nextHandleId
        nextHandleId += 1
        activeProviders[handleId] = result.itemProvider
        
        on_photo_picker_result(cb_id, handleId)
    }
}
#endif
