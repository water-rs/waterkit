#if os(iOS)
import UIKit
import MobileCoreServices
typealias UXImage = UIImage
typealias UXPasteboard = UIPasteboard

func getPasteboard() -> UIPasteboard {
    return UIPasteboard.general
}
#elseif os(macOS)
import AppKit
typealias UXImage = NSImage
typealias UXPasteboard = NSPasteboard

func getPasteboard() -> NSPasteboard {
    return NSPasteboard.general
}
#endif

public func clipboard_get_text() -> Optional<String> {
    #if os(iOS)
    return UIPasteboard.general.string
    #elseif os(macOS)
    return NSPasteboard.general.string(forType: .string)
    #endif
}

public func clipboard_set_text(text: String) {
    #if os(iOS)
    UIPasteboard.general.string = text
    #elseif os(macOS)
    let pb = NSPasteboard.general
    pb.clearContents()
    pb.setString(text, forType: .string)
    #endif
}

public func clipboard_get_image() -> SwiftImageData {
    #if os(iOS)
    guard let image = UIPasteboard.general.image else {
        return SwiftImageData(width: 0, height: 0, bytes: RustVec(), is_valid: false)
    }
    guard let cgImage = image.cgImage else {
         return SwiftImageData(width: 0, height: 0, bytes: RustVec(), is_valid: false)
    }
    #elseif os(macOS)
    let pb = NSPasteboard.general
    // Try to read standard image types
    guard let image = NSImage(pasteboard: pb) else {
        return SwiftImageData(width: 0, height: 0, bytes: RustVec(), is_valid: false)
    }
    // Convert to CGImage
    var rect = CGRect(origin: .zero, size: image.size)
    guard let cgImage = image.cgImage(forProposedRect: &rect, context: nil, hints: nil) else {
        return SwiftImageData(width: 0, height: 0, bytes: RustVec(), is_valid: false)
    }
    #endif
    
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
        return SwiftImageData(width: 0, height: 0, bytes: RustVec(), is_valid: false)
    }
    
    context.draw(cgImage, in: CGRect(x: 0, y: 0, width: CGFloat(width), height: CGFloat(height)))
    
    let rustVec = RustVec<UInt8>()
    for byte in rawData {
        rustVec.push(value: byte)
    }
    
    return SwiftImageData(width: UInt(width), height: UInt(height), bytes: rustVec, is_valid: true)
}

public func clipboard_set_image(image: SwiftImageData) {
    if !image.is_valid { return }
    let width = Int(image.width)
    let height = Int(image.height)
    
    // Copy data
    var data = Data(capacity: width * height * 4)
    for i in 0..<image.bytes.len() {
        if let byte = image.bytes.get(index: i) {
             data.append(byte)
        }
    }
    
    let colorSpace = CGColorSpaceCreateDeviceRGB()
    let bitmapInfo = CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue)
    
    guard let provider = CGDataProvider(data: data as CFData) else { return }
    
    guard let cgImage = CGImage(width: width,
                                height: height,
                                bitsPerComponent: 8,
                                bitsPerPixel: 32,
                                bytesPerRow: width * 4,
                                space: colorSpace,
                                bitmapInfo: bitmapInfo,
                                provider: provider,
                                decode: nil,
                                shouldInterpolate: false,
                                intent: .defaultIntent) else { return }

    #if os(iOS)
    let uiImage = UIImage(cgImage: cgImage)
    UIPasteboard.general.image = uiImage
    #elseif os(macOS)
    let nsImage = NSImage(cgImage: cgImage, size: NSSize(width: width, height: height))
    let pb = NSPasteboard.general
    pb.clearContents()
    pb.writeObjects([nsImage])
    #endif
}
