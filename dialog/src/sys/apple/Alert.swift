import Foundation
#if os(iOS)
import UIKit
import PhotosUI
import UniformTypeIdentifiers
#elseif os(macOS)
import AppKit
#endif

func show_alert_bridge(title: RustStr, message: RustStr, type_str: RustStr, cb_id: UInt64) {
    let titleStr = title.toString()
    let messageStr = message.toString()
    
    DispatchQueue.main.async {
        #if os(iOS)
        guard let topVC = getTopViewController() else {
            on_dialog_result(cb_id, false)
            return
        }
        
        let alert = UIAlertController(title: titleStr, message: messageStr, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "OK", style: .default) { _ in
            on_dialog_result(cb_id, true)
        })
        topVC.present(alert, animated: true)
        #elseif os(macOS)
        let alert = NSAlert()
        alert.messageText = titleStr
        alert.informativeText = messageStr
        alert.alertStyle = .informational // simplified mapping
        alert.addButton(withTitle: "OK")
        let _ = alert.runModal()
        on_dialog_result(cb_id, true)
        #endif
    }
}

func show_confirm_bridge(title: RustStr, message: RustStr, type_str: RustStr, cb_id: UInt64) {
    let titleStr = title.toString()
    let messageStr = message.toString()
    
    DispatchQueue.main.async {
        #if os(iOS)
        guard let topVC = getTopViewController() else {
            on_dialog_result(cb_id, false)
            return
        }
        
        let alert = UIAlertController(title: titleStr, message: messageStr, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "OK", style: .default) { _ in
            on_dialog_result(cb_id, true)
        })
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel) { _ in
            on_dialog_result(cb_id, false)
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
        on_dialog_result(cb_id, response == .alertFirstButtonReturn)
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
private var activeFilePickerDelegates: [UInt64: Any] = [:]
private var nextHandleId: UInt64 = 1
private let pathListSeparator = "\u{0000}"

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

func show_open_file_bridge(extensions_csv: RustStr, cb_id: UInt64) {
    let extensions = extensions_csv
        .toString()
        .split(separator: ",")
        .map { $0.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() }
        .filter { !$0.isEmpty }

    DispatchQueue.main.async {
        guard let topVC = getTopViewController() else {
            on_open_file_result(cb_id, nil as String?)
            return
        }

        let documentTypes: [String]
        if #available(iOS 14.0, *) {
            let mapped = extensions.compactMap { ext in
                UTType(filenameExtension: ext)?.identifier
            }
            documentTypes = mapped.isEmpty ? [UTType.item.identifier] : mapped
        } else {
            documentTypes = ["public.data"]
        }

        let delegate = FilePickerDelegate(cbId: cb_id)
        activeFilePickerDelegates[cb_id] = delegate

        let picker = UIDocumentPickerViewController(documentTypes: documentTypes, in: .import)
        picker.delegate = delegate
        picker.allowsMultipleSelection = false
        topVC.present(picker, animated: true)
    }
}

func show_open_multiple_files_bridge(extensions_csv: RustStr, cb_id: UInt64) {
    let extensions = extensions_csv
        .toString()
        .split(separator: ",")
        .map { $0.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() }
        .filter { !$0.isEmpty }

    DispatchQueue.main.async {
        guard let topVC = getTopViewController() else {
            on_open_multiple_files_result(cb_id, nil as String?)
            return
        }

        let documentTypes: [String]
        if #available(iOS 14.0, *) {
            let mapped = extensions.compactMap { ext in
                UTType(filenameExtension: ext)?.identifier
            }
            documentTypes = mapped.isEmpty ? [UTType.item.identifier] : mapped
        } else {
            documentTypes = ["public.data"]
        }

        let delegate = MultiFilePickerDelegate(cbId: cb_id)
        activeFilePickerDelegates[cb_id] = delegate

        let picker = UIDocumentPickerViewController(documentTypes: documentTypes, in: .import)
        picker.delegate = delegate
        picker.allowsMultipleSelection = true
        topVC.present(picker, animated: true)
    }
}

func load_media_bridge(handle_id: UInt64, cb_id: UInt64) {
    DispatchQueue.main.async {
        guard let provider = activeProviders[handle_id] else {
            on_load_media_result(cb_id, nil as String?)
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
             on_load_media_result(cb_id, nil as String?)
        }
    }
}

private func loadFile(_ provider: NSItemProvider, type: String, cb_id: UInt64) {
    provider.loadFileRepresentation(forTypeIdentifier: type) { url, error in
        guard let url = url else {
            on_load_media_result(cb_id, nil as String?)
            return
        }

        on_load_media_result(cb_id, copyToTemporaryLocation(url))
    }
}

private func copyToTemporaryLocation(_ sourceUrl: URL) -> String? {
    let hasScopedAccess = sourceUrl.startAccessingSecurityScopedResource()
    defer {
        if hasScopedAccess {
            sourceUrl.stopAccessingSecurityScopedResource()
        }
    }

    let tmpDir = FileManager.default.temporaryDirectory
    let extensionSuffix = sourceUrl.pathExtension.isEmpty ? "" : ".\(sourceUrl.pathExtension)"
    let dstUrl = tmpDir.appendingPathComponent(UUID().uuidString + extensionSuffix)

    do {
        if FileManager.default.fileExists(atPath: dstUrl.path) {
            try FileManager.default.removeItem(at: dstUrl)
        }
        try FileManager.default.copyItem(at: sourceUrl, to: dstUrl)
        return dstUrl.path
    } catch {
        return nil
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

final class FilePickerDelegate: NSObject, UIDocumentPickerDelegate {
    let cbId: UInt64

    init(cbId: UInt64) {
        self.cbId = cbId
    }

    private func finish(_ path: String?) {
        on_open_file_result(cbId, path)
        activeFilePickerDelegates.removeValue(forKey: cbId)
    }

    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
        finish(nil)
    }

    func documentPicker(
        _ controller: UIDocumentPickerViewController,
        didPickDocumentsAt urls: [URL]
    ) {
        guard let first = urls.first else {
            finish(nil)
            return
        }
        finish(copyToTemporaryLocation(first))
    }
}

final class MultiFilePickerDelegate: NSObject, UIDocumentPickerDelegate {
    let cbId: UInt64

    init(cbId: UInt64) {
        self.cbId = cbId
    }

    private func finish(_ paths: String?) {
        on_open_multiple_files_result(cbId, paths)
        activeFilePickerDelegates.removeValue(forKey: cbId)
    }

    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
        finish(nil)
    }

    func documentPicker(
        _ controller: UIDocumentPickerViewController,
        didPickDocumentsAt urls: [URL]
    ) {
        let copied = urls.compactMap(copyToTemporaryLocation)
        if copied.isEmpty {
            finish(nil)
            return
        }
        finish(copied.joined(separator: pathListSeparator))
    }
}
#endif
