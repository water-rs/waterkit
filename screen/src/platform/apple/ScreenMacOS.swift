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

/// Get the total number of unique frames captured by ScreenCaptureKit
public func get_frame_count() -> UInt32 {
    frameLock.lock()
    defer { frameLock.unlock() }
    return frameSequence
}

/// Reset the frame counter (call before timing test)
public func reset_frame_count() {
    frameLock.lock()
    frameSequence = 0
    frameLock.unlock()
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

// MARK: - High-Performance Stream Capturer for 30+ FPS

/// Raw frame result from ScreenCaptureKit
fileprivate var lastCapturedFrame: [UInt8]? = nil
fileprivate var frameWidth: UInt32 = 0
fileprivate var frameHeight: UInt32 = 0
fileprivate var frameSequence: UInt32 = 0  // Tracks unique frames delivered
fileprivate var lastReadSequence: UInt32 = 0  // Last sequence read by Rust
fileprivate let frameLock = NSLock()
fileprivate var streamCapturer: SCKStreamCapturer? = nil
fileprivate var rawFrameCaptureEnabled: Bool = true

// MARK: - Zero-Copy IOSurface Storage
import IOSurface

/// Stores the latest IOSurface for zero-copy GPU access
fileprivate var lastIOSurface: IOSurfaceRef? = nil
fileprivate var ioSurfaceSequence: UInt32 = 0

@available(macOS 12.3, *)
class SCKStreamCapturer: NSObject, SCStreamOutput, SCStreamDelegate {
    private var stream: SCStream?
    private var isRunning = false
    
    func start(completion: @escaping (Bool) -> Void) {
        guard !isRunning else {
            completion(true)
            return
        }
        
        // Get shareable content
        SCShareableContent.getExcludingDesktopWindows(false, onScreenWindowsOnly: true) { [weak self] content, error in
            guard let self = self, let content = content, error == nil else {
                print("Failed to get shareable content: \(error?.localizedDescription ?? "unknown")")
                completion(false)
                return
            }
            
            guard let display = content.displays.first else {
                print("No display found")
                completion(false)
                return
            }
            
            // Create filter for the main display
            let filter = SCContentFilter(display: display, excludingWindows: [])
            
            // Configure for maximum speed capture
            let config = SCStreamConfiguration()
            config.width = Int(display.width)
            config.height = Int(display.height)
            config.minimumFrameInterval = CMTime(value: 1, timescale: 240) // Request 240fps
            config.queueDepth = 10  // Larger queue to prevent drops
            config.pixelFormat = kCVPixelFormatType_32BGRA
            config.showsCursor = false  // Skip cursor compositing
            if #available(macOS 13.0, *) {
                config.capturesAudio = false
            }
            
            do {
                self.stream = SCStream(filter: filter, configuration: config, delegate: self)
                try self.stream!.addStreamOutput(self, type: .screen, sampleHandlerQueue: DispatchQueue.global(qos: .userInteractive))
                
                self.stream!.startCapture { error in
                    if let error = error {
                        print("Stream start failed: \(error)")
                        completion(false)
                    } else {
                        self.isRunning = true
                        completion(true)
                    }
                }
            } catch {
                print("Stream creation failed: \(error)")
                completion(false)
            }
        }
    }
    
    func stop() {
        guard isRunning else { return }
        stream?.stopCapture()
        isRunning = false
    }
    
    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, let pixelBuffer = sampleBuffer.imageBuffer else { return }
        
        let width = CVPixelBufferGetWidth(pixelBuffer)
        let height = CVPixelBufferGetHeight(pixelBuffer)
        
        // Get IOSurface for zero-copy GPU access
        let ioSurface = CVPixelBufferGetIOSurface(pixelBuffer)?.takeUnretainedValue()
        
        // Store frame
        frameLock.lock()
        frameWidth = UInt32(width)
        frameHeight = UInt32(height)
        frameSequence += 1
        
        if rawFrameCaptureEnabled {
            CVPixelBufferLockBaseAddress(pixelBuffer, .readOnly)
            defer { CVPixelBufferUnlockBaseAddress(pixelBuffer, .readOnly) }
            
            let bytesPerRow = CVPixelBufferGetBytesPerRow(pixelBuffer)
            guard let baseAddress = CVPixelBufferGetBaseAddress(pixelBuffer) else {
                frameLock.unlock()
                return
            }

            // Copy frame data (handle stride)
            let expectedBytesPerRow = width * 4
            var rawData = [UInt8](repeating: 0, count: width * height * 4)
            
            if bytesPerRow == expectedBytesPerRow {
                rawData.withUnsafeMutableBytes { dest in
                    dest.copyBytes(from: UnsafeRawBufferPointer(start: baseAddress, count: width * height * 4))
                }
            } else {
                let src = baseAddress.assumingMemoryBound(to: UInt8.self)
                for row in 0..<height {
                    let srcRow = src.advanced(by: row * bytesPerRow)
                    let dstOffset = row * expectedBytesPerRow
                    rawData.withUnsafeMutableBytes { dest in
                        dest.baseAddress!.advanced(by: dstOffset).copyMemory(from: srcRow, byteCount: expectedBytesPerRow)
                    }
                }
            }
            lastCapturedFrame = rawData
        } else {
            lastCapturedFrame = nil
        }

        if let surface = ioSurface {
            lastIOSurface = surface
            ioSurfaceSequence += 1
        }
        frameLock.unlock()
    }
    
    func stream(_ stream: SCStream, didStopWithError error: Error) {
        print("Stream stopped: \(error)")
        isRunning = false
    }
}

/// Initialize the ScreenCaptureKit stream for high-speed capture
public func init_sck_stream() -> Bool {
    if #available(macOS 12.3, *) {
        let capturer = SCKStreamCapturer()
        
        // Use a semaphore to wait for callback-based start
        var success = false
        let sem = DispatchSemaphore(value: 0)
        
        capturer.start { result in
            success = result
            sem.signal()
        }
        
        // Wait up to 2 seconds for stream to start
        _ = sem.wait(timeout: .now() + 2.0)
        
        if success {
            streamCapturer = capturer
        }
        return success
    }
    return false
}

/// Stop the ScreenCaptureKit stream
public func stop_sck_stream() {
    streamCapturer?.stop()
    streamCapturer = nil
}

/// Get the latest captured frame as raw BGRA bytes with dimensions
public func get_latest_frame() -> RustVec<UInt8> {
    frameLock.lock()
    defer { frameLock.unlock() }
    
    if frameWidth == 0 || frameHeight == 0 {
        return RustVec()
    }
    
    // Only return dimensions for now (fast timing test)
    let vec = RustVec<UInt8>()
    
    // Width (4 bytes LE)
    vec.push(value: UInt8(frameWidth & 0xFF))
    vec.push(value: UInt8((frameWidth >> 8) & 0xFF))
    vec.push(value: UInt8((frameWidth >> 16) & 0xFF))
    vec.push(value: UInt8((frameWidth >> 24) & 0xFF))
    
    // Height (4 bytes LE)
    vec.push(value: UInt8(frameHeight & 0xFF))
    vec.push(value: UInt8((frameHeight >> 8) & 0xFF))
    vec.push(value: UInt8((frameHeight >> 16) & 0xFF))
    vec.push(value: UInt8((frameHeight >> 24) & 0xFF))
    
    if lastCapturedFrame == nil {
        // Mark as 'dimensions only' by setting 9th byte
        vec.push(value: 0xFF)
        return vec
    }

    // Mark as 'dimensions only' by setting 9th byte
    vec.push(value: 0xFF)
    return vec
}

/// Enable or disable raw frame copy to CPU memory.
public func set_raw_frame_capture_enabled(enabled: Bool) {
    frameLock.lock()
    rawFrameCaptureEnabled = enabled
    if !enabled {
        lastCapturedFrame = nil
    }
    frameLock.unlock()
}

/// Get the raw pointer to the current IOSurface for zero-copy GPU access.
/// Returns 0 if no IOSurface is available.
public func get_iosurface_ptr() -> UInt64 {
    frameLock.lock()
    defer { frameLock.unlock() }
    
    guard let surface = lastIOSurface else {
        return 0
    }
    
    // Return the raw pointer as UInt64
    return UInt64(UInt(bitPattern: Unmanaged.passUnretained(surface as AnyObject).toOpaque()))
}

/// Get the IOSurface sequence number to detect new frames
public func get_iosurface_sequence() -> UInt32 {
    frameLock.lock()
    defer { frameLock.unlock() }
    return ioSurfaceSequence
}
