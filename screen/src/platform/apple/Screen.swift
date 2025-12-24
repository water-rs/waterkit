import Foundation
import UIKit // For iOS
// import AppKit // For macOS native if we were not using xcap

// Note: xcap handles macOS. This file is primarily for iOS if we use target_os logic correctly.
// But we might be compiling for both in the bridge.
// However, the rust side `apple.rs` is cfg(any(ios, macos)).
// If we want to use xcap for macOS, we should exclude macos from `apple.rs` or use cfg in `apple.rs`.
// Revisiting `mod.rs`:
// #[cfg(any(target_os = "macos", ...))] mod desktop;
// #[cfg(any(target_os = "ios"))] mod apple;
//
// So `apple.rs` is ONLY for iOS.
// Thus we can import UIKit safely.

public func get_screen_brightness() -> Float {
    return Float(UIScreen.main.brightness)
}

public func set_screen_brightness(value: Float) {
    UIScreen.main.brightness = CGFloat(value)
}

public func capture_main_screen() -> RustVec<UInt8>? {
    // Capture the snapshot of the key window
    // This needs to run on main thread
    var result: RustVec<UInt8>? = nil
    
    DispatchQueue.main.sync {
        guard let window = UIApplication.shared.windows.first(where: { $0.isKeyWindow }) else {
            return
        }
        
        let renderer = UIGraphicsImageRenderer(bounds: window.bounds)
        let image = renderer.image { ctx in
            window.drawHierarchy(in: window.bounds, afterScreenUpdates: true)
        }
        
        if let data = image.pngData() {
            // Convert Data to RustVec
            // Since swift-bridge doesn't support Data -> RustVec direct conversion effectively without helper?
            // Actually it does support returning Option<Vec<u8>> as RustVec?
            // We need to construct it.
            // Simplified: return nil for now as I recall swift-bridge vector return is tricky manually without specific integration.
            // But let's try populating a buffer.
            
            // For now, to avoid complexity of constructing RustVec manually if not auto-bridged:
            // "swift-bridge supports returning Vec<u8> -> RustVec<UInt8>"
            let vec = RustVec<UInt8>()
            for byte in data {
                vec.push(value: byte)
            }
            result = vec
        }
    }
    
    return result
}
