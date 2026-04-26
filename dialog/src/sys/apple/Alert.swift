import Foundation
#if os(iOS)
import UIKit
import Photos
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
// Keep picker selections alive for handles
private struct PhotoPickerSelection {
    let itemProvider: NSItemProvider
    let assetIdentifier: String?
}

private var activeSelections: [UInt64: PhotoPickerSelection] = [:]
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

func load_photo_media_bridge(handle_id: UInt64, media_type: RustStr, cb_id: UInt64) {
    let requestedMediaType = media_type.toString()

    DispatchQueue.main.async {
        guard let selection = activeSelections[handle_id] else {
            on_load_media_result(cb_id, nil as String?)
            return
        }

        if requestedMediaType == "livephoto" {
            loadLivePhoto(selection, cbId: cb_id)
            return
        }

        if selection.itemProvider.hasItemConformingToTypeIdentifier(UTType.movie.identifier) {
            loadSingleMedia(
                selection.itemProvider,
                type: UTType.movie.identifier,
                kind: "video",
                cbId: cb_id
            )
        } else if selection.itemProvider.hasItemConformingToTypeIdentifier(UTType.image.identifier) {
            loadSingleMedia(
                selection.itemProvider,
                type: UTType.image.identifier,
                kind: "image",
                cbId: cb_id
            )
        } else {
            on_load_media_result(cb_id, nil as String?)
        }
    }
}

private func loadSingleMedia(_ provider: NSItemProvider, type: String, kind: String, cbId: UInt64) {
    provider.loadFileRepresentation(forTypeIdentifier: type) { url, error in
        guard let url = url else {
            on_load_media_result(cbId, nil as String?)
            return
        }

        let copied = copyToTemporaryLocation(
            url,
            preferredFilename: provider.suggestedName,
            typeIdentifier: type
        )
        on_load_media_result(cbId, copied.map { encodeLoadedMediaPayload(kind: kind, paths: [$0]) })
    }
}

private func loadLivePhoto(_ selection: PhotoPickerSelection, cbId: UInt64) {
    guard let assetIdentifier = selection.assetIdentifier else {
        on_load_media_result(cbId, nil as String?)
        return
    }

    let assets = PHAsset.fetchAssets(withLocalIdentifiers: [assetIdentifier], options: nil)
    guard let asset = assets.firstObject else {
        on_load_media_result(cbId, nil as String?)
        return
    }

    let resources = PHAssetResource.assetResources(for: asset)
    guard
        let imageResource = preferredImageResource(from: resources),
        let videoResource = preferredVideoResource(from: resources)
    else {
        on_load_media_result(cbId, nil as String?)
        return
    }

    let options = PHAssetResourceRequestOptions()
    options.isNetworkAccessAllowed = true

    let group = DispatchGroup()
    var imagePath: String?
    var videoPath: String?

    group.enter()
    writeAssetResourceToTemporaryLocation(imageResource, options: options) { path in
        imagePath = path
        group.leave()
    }

    group.enter()
    writeAssetResourceToTemporaryLocation(videoResource, options: options) { path in
        videoPath = path
        group.leave()
    }

    group.notify(queue: .main) {
        guard let imagePath, let videoPath else {
            on_load_media_result(cbId, nil as String?)
            return
        }

        on_load_media_result(
            cbId,
            encodeLoadedMediaPayload(kind: "livephoto", paths: [imagePath, videoPath])
        )
    }
}

private func writeAssetResourceToTemporaryLocation(
    _ resource: PHAssetResource,
    options: PHAssetResourceRequestOptions,
    completion: @escaping (String?) -> Void
) {
    let destination = temporaryDestinationURL(
        preferredFilename: resource.originalFilename,
        typeIdentifier: resource.uniformTypeIdentifier
    )

    PHAssetResourceManager.default().writeData(
        for: resource,
        toFile: destination,
        options: options
    ) { error in
        if error != nil {
            completion(nil)
        } else {
            completion(destination.path)
        }
    }
}

private func preferredImageResource(from resources: [PHAssetResource]) -> PHAssetResource? {
    let preferredTypes: [PHAssetResourceType] = [.fullSizePhoto, .photo, .alternatePhoto]
    for type in preferredTypes {
        if let resource = resources.first(where: { $0.type == type }) {
            return resource
        }
    }
    return nil
}

private func preferredVideoResource(from resources: [PHAssetResource]) -> PHAssetResource? {
    let preferredTypes: [PHAssetResourceType] = [.fullSizePairedVideo, .pairedVideo, .video]
    for type in preferredTypes {
        if let resource = resources.first(where: { $0.type == type }) {
            return resource
        }
    }
    return nil
}

private func encodeLoadedMediaPayload(kind: String, paths: [String]) -> String {
    ([kind] + paths).joined(separator: pathListSeparator)
}

private func copyToTemporaryLocation(
    _ sourceUrl: URL,
    preferredFilename: String? = nil,
    typeIdentifier: String? = nil
) -> String? {
    let hasScopedAccess = sourceUrl.startAccessingSecurityScopedResource()
    defer {
        if hasScopedAccess {
            sourceUrl.stopAccessingSecurityScopedResource()
        }
    }

    let dstUrl = temporaryDestinationURL(
        sourceUrl: sourceUrl,
        preferredFilename: preferredFilename,
        typeIdentifier: typeIdentifier
    )

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

private func temporaryDestinationURL(
    sourceUrl: URL? = nil,
    preferredFilename: String? = nil,
    typeIdentifier: String? = nil
) -> URL {
    let tmpDir = FileManager.default.temporaryDirectory
    let extensionSuffix = resolvedFilenameExtension(
        sourceUrl: sourceUrl,
        preferredFilename: preferredFilename,
        typeIdentifier: typeIdentifier
    )
    let fileName = extensionSuffix.isEmpty ? UUID().uuidString : "\(UUID().uuidString).\(extensionSuffix)"
    return tmpDir.appendingPathComponent(fileName)
}

private func resolvedFilenameExtension(
    sourceUrl: URL?,
    preferredFilename: String?,
    typeIdentifier: String?
) -> String {
    if let sourceUrl, !sourceUrl.pathExtension.isEmpty {
        return sourceUrl.pathExtension
    }

    if let preferredFilename, !preferredFilename.isEmpty {
        let ext = URL(fileURLWithPath: preferredFilename).pathExtension
        if !ext.isEmpty {
            return ext
        }
    }

    if let typeIdentifier {
        if #available(iOS 14.0, *) {
            if let type = UTType(importedAs: typeIdentifier).preferredFilenameExtension {
                return type
            }
        }
    }

    return ""
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
        
        // Store provider + asset metadata and return handle
        let handleId = nextHandleId
        nextHandleId += 1
        activeSelections[handleId] = PhotoPickerSelection(
            itemProvider: result.itemProvider,
            assetIdentifier: result.assetIdentifier
        )
        
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
        let copied = urls.compactMap { url in
            copyToTemporaryLocation(url)
        }
        if copied.isEmpty {
            finish(nil)
            return
        }
        finish(copied.joined(separator: pathListSeparator))
    }
}
#endif
