import Foundation
import ScreenCaptureKit
import Cocoa

// Note: apple.rs bridge uses `show_picker_and_capture` for macOS.

// We need a class to handle the stream output delegate.
class SCKHandler: NSObject, SCStreamOutput, SCStreamDelegate {
    static let shared = SCKHandler()
    
    var stream: SCStream?
    
    func stream(_ stream: SCStream, didStopWithError error: Error) {
        print("Stream stopped with error: \(error)")
        on_picker_result(RustVec())
    }
    
    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, let imageBuffer = sampleBuffer.imageBuffer else { return }
        
        // Convert CVPixelBuffer/ImageBuffer to PNG
        let ciImage = CIImage(cvImageBuffer: imageBuffer)
        let context = CIContext()
        if let cgImage = context.createCGImage(ciImage, from: ciImage.extent) {
            let bitmapRep = NSBitmapImageRep(cgImage: cgImage)
            if let pngData = bitmapRep.representation(using: .png, properties: [:]) {
                // Convert Data to RustVec
                let vec = RustVec<UInt8>()
                for byte in pngData {
                    vec.push(value: byte)
                }
                on_picker_result(vec)
            } else {
                 on_picker_result(RustVec())
            }
        } else {
             on_picker_result(RustVec())
        }
        
        // Stop stream after one frame
        stream.stopCapture()
    }
}


@available(macOS 14.0, *)
class PickerDelegate: NSObject, SCContentSharingPickerObserver {
    func contentSharingPicker(_ picker: SCContentSharingPicker, didCancelFor stream: SCStream?) {
        print("Picker cancelled")
        on_picker_result(RustVec())
    }
    
    func contentSharingPicker(_ picker: SCContentSharingPicker, didUpdateWith filter: SCContentFilter, for stream: SCStream?) {
        print("Picker did update with filter")
        startCapture(filter: filter)
    }
    
    func contentSharingPickerStartDidFailWithError(_ error: Error) {
        print("Picker failed: \(error)")
        on_picker_result(RustVec())
    }
    
    func startCapture(filter: SCContentFilter) {
        let configuration = SCStreamConfiguration()
        configuration.width = 0 // Auto?
        configuration.height = 0
        configuration.minimumFrameInterval = CMTime(value: 1, timescale: 60)
        configuration.queueDepth = 1
        
        do {
            let stream = SCStream(filter: filter, configuration: configuration, delegate: SCKHandler.shared)
            try stream.addStreamOutput(SCKHandler.shared, type: .screen, sampleHandlerQueue: DispatchQueue.main)
            SCKHandler.shared.stream = stream
            
            stream.startCapture { error in
                if let error = error {
                    print("Stream start failed: \(error)")
                    on_picker_result(RustVec())
                }
            }
        } catch {
             print("Stream creation failed: \(error)")
             on_picker_result(RustVec())
        }
    }
}
 
// Global delegate
var pickerDelegate: Any? = nil

public func show_picker_and_capture() {
    if #available(macOS 14.0, *) {
        DispatchQueue.main.async {
            let picker = SCContentSharingPicker.shared
            let delegate = PickerDelegate()
            pickerDelegate = delegate // Keep alive
            picker.add(delegate)
            picker.isActive = true
            picker.present()
        }
    } else {
        print("Picker requires macOS 14.0+")
        on_picker_result(RustVec())
    }
}

// Stubs for iOS function used in rust (but not called on macOS side if setup correctly)
// But swift-bridge might generate them for both if compiled together.
// The Rust side `apple.rs` calls these.
// Since `get_screen_brightness` is `extern "Swift"`, we must implement it if the bridge expects it.
// Even if unused on macOS, linker might complain if missing.

public func get_screen_brightness() -> Float {
    return 1.0 // Stub for macOS
}

public func set_screen_brightness(value: Float) {
    // Stub
}

public func capture_main_screen() -> RustVec<UInt8> {
    return RustVec() // Stub
}
