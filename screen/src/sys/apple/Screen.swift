import Foundation
import UIKit

// MARK: - Screenshot Capture

/// Capture the main screen and return encoded image data.
/// - Parameter format: 0 = PNG, 1 = AVIF, 2 = HEIF
public func capture_screenshot(format: UInt8) -> RustVec<UInt8> {
    var result: RustVec<UInt8>? = nil

    DispatchQueue.main.sync {
        guard let window = UIApplication.shared.windows.first(where: { $0.isKeyWindow }) else {
            return
        }

        let renderer = UIGraphicsImageRenderer(bounds: window.bounds)
        let image = renderer.image { ctx in
            window.drawHierarchy(in: window.bounds, afterScreenUpdates: true)
        }

        var data: Data?
        switch format {
        case 1: // AVIF
            data = encodeAsAVIF(image)
        case 2: // HEIF
            data = encodeAsHEIF(image)
        default: // PNG
            data = image.pngData()
        }

        if let data = data {
            let vec = RustVec<UInt8>()
            for byte in data {
                vec.push(value: byte)
            }
            result = vec
        }
    }

    return result ?? RustVec()
}

private func encodeAsHEIF(_ image: UIImage) -> Data? {
    guard let cgImage = image.cgImage else { return nil }

    let data = NSMutableData()
    let uti = "public.heic" as CFString
    guard let dest = CGImageDestinationCreateWithData(data, uti, 1, nil) else {
        return nil
    }

    CGImageDestinationAddImage(dest, cgImage, nil)
    if CGImageDestinationFinalize(dest) {
        return data as Data
    }
    return nil
}

private func encodeAsAVIF(_ image: UIImage) -> Data? {
    guard let cgImage = image.cgImage else { return nil }

    let data = NSMutableData()
    let uti = "public.avif" as CFString
    guard let dest = CGImageDestinationCreateWithData(data, uti, 1, nil) else {
        return nil
    }

    CGImageDestinationAddImage(dest, cgImage, nil)
    if CGImageDestinationFinalize(dest) {
        return data as Data
    }
    return nil
}

// MARK: - Brightness Control

public func get_screen_brightness() -> Float {
    return Float(UIScreen.main.brightness)
}

public func set_screen_brightness(value: Float) {
    UIScreen.main.brightness = CGFloat(value)
}

// MARK: - Screen Stream (Not supported on iOS)

public func init_screen_stream(display_id: UInt32, target_fps: UInt32, show_cursor: Bool) -> Bool {
    return false
}

public func stop_screen_stream() {
    // No-op
}

public func get_iosurface_ptr() -> UInt64 {
    return 0
}

public func get_iosurface_sequence() -> UInt32 {
    return 0
}

public func get_frame_width() -> UInt32 {
    return 0
}

public func get_frame_height() -> UInt32 {
    return 0
}

public func get_frame_timestamp_ns() -> UInt64 {
    return 0
}
