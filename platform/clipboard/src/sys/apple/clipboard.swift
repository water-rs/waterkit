// iOS-only clipboard implementation
// macOS uses clipboard-rs instead

import UIKit
import MobileCoreServices
import UniformTypeIdentifiers

// MARK: - Type Detection (Query)

public func clipboard_has_text() -> Bool {
    return UIPasteboard.general.hasStrings
}

public func clipboard_has_html() -> Bool {
    return UIPasteboard.general.contains(pasteboardTypes: [UTType.html.identifier])
}

public func clipboard_has_image() -> Bool {
    return UIPasteboard.general.hasImages
}

public func clipboard_has_files() -> Bool {
    // Check if there's a file:// URL
    if let url = UIPasteboard.general.url {
        return url.isFileURL
    }
    return false
}

// MARK: - Read Operations

public func clipboard_get_text() -> Optional<String> {
    return UIPasteboard.general.string
}

public func clipboard_get_html() -> SwiftBinaryData {
    guard let data = UIPasteboard.general.data(forPasteboardType: UTType.html.identifier) else {
        return SwiftBinaryData(bytes: RustVec(), is_valid: false)
    }
    // Decoded on the Rust side, which reports bytes that are not UTF-8.
    return SwiftBinaryData(bytes: rustVec(data), is_valid: true)
}

/// No image on the pasteboard.
private func noImage() -> SwiftImageData {
    return SwiftImageData(width: 0, height: 0, bytes: RustVec(), is_valid: false, error: nil)
}

/// An image on the pasteboard that could not be converted to RGBA pixels.
private func unreadableImage(_ reason: String) -> SwiftImageData {
    return SwiftImageData(
        width: 0, height: 0, bytes: RustVec(), is_valid: false, error: reason.intoRustString())
}

public func clipboard_get_image() -> SwiftImageData {
    guard let image = UIPasteboard.general.image else {
        return noImage()
    }
    guard let cgImage = image.cgImage else {
        return unreadableImage("the pasteboard image has no bitmap (CGImage) representation")
    }

    let width = cgImage.width
    let height = cgImage.height

    let bytesPerPixel = 4
    let bytesPerRow = bytesPerPixel * width
    let bitsPerComponent = 8

    var rawData = [UInt8](repeating: 0, count: width * height * 4)

    let colorSpace = CGColorSpaceCreateDeviceRGB()
    let bitmapInfo = CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue)

    guard let context = CGContext(data: &rawData,
                                  width: width,
                                  height: height,
                                  bitsPerComponent: bitsPerComponent,
                                  bytesPerRow: bytesPerRow,
                                  space: colorSpace,
                                  bitmapInfo: bitmapInfo.rawValue) else {
        return unreadableImage(
            "failed to create a \(width)x\(height) RGBA bitmap context for the pasteboard image")
    }

    context.draw(cgImage, in: CGRect(x: 0, y: 0, width: CGFloat(width), height: CGFloat(height)))

    return SwiftImageData(
        width: UInt(width), height: UInt(height), bytes: rustVec(rawData), is_valid: true, error: nil)
}

public func clipboard_get_file_url() -> Optional<String> {
    if let url = UIPasteboard.general.url, url.isFileURL {
        return url.absoluteString
    }
    return nil
}

public func clipboard_get_binary(mime: RustString) -> SwiftBinaryData {
    let mimeType = mime.toString()

    // Try to get data for the MIME type directly
    guard let data = UIPasteboard.general.data(forPasteboardType: mimeType) else {
        return SwiftBinaryData(bytes: RustVec(), is_valid: false)
    }

    return SwiftBinaryData(bytes: rustVec(data), is_valid: true)
}

/// `bytes` copied into a vector Rust owns.
private func rustVec<Bytes: Sequence>(_ bytes: Bytes) -> RustVec<UInt8> where Bytes.Element == UInt8 {
    let vec = RustVec<UInt8>()
    for byte in bytes {
        vec.push(value: byte)
    }
    return vec
}

// MARK: - Write Operations

public func clipboard_set_text(text: RustString) {
    UIPasteboard.general.string = text.toString()
}

public func clipboard_set_html(html: RustString, alt_text: RustString) {
    let htmlString = html.toString()
    let altString = alt_text.toString()

    var items: [[String: Any]] = []

    // Set HTML data
    if let htmlData = htmlString.data(using: .utf8) {
        items.append([UTType.html.identifier: htmlData])
    }

    // Set plain text as fallback
    if !altString.isEmpty {
        items.append([UTType.plainText.identifier: altString])
    }

    UIPasteboard.general.items = items
}

public func clipboard_set_image_from_path(path: RustString) -> Bool {
    let filePath = path.toString()
    guard let image = UIImage(contentsOfFile: filePath) else {
        return false
    }
    UIPasteboard.general.image = image
    return true
}

public func clipboard_set_file_url(url: RustString) {
    if let urlObj = URL(string: url.toString()) {
        UIPasteboard.general.url = urlObj
    }
}

public func clipboard_set_binary(data: RustVec<UInt8>, mime: RustString) {
    let mimeType = mime.toString()

    var bytes = Data(capacity: Int(data.len()))
    for i in 0..<Int(data.len()) {
        if let byte = data.get(index: UInt(i)) {
            bytes.append(byte)
        }
    }

    UIPasteboard.general.setData(bytes, forPasteboardType: mimeType)
}

// MARK: - Control

public func clipboard_clear() {
    UIPasteboard.general.items = []
}

public func clipboard_get_change_count() -> Int64 {
    return Int64(UIPasteboard.general.changeCount)
}
