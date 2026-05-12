import Foundation
import ScreenCaptureKit
import Cocoa
import IOSurface
import ImageIO
import IOKit
import IOKit.graphics

// MARK: - Screenshot Capture

/// Capture the primary screen and return encoded image data.
/// - Parameter format: 0 = PNG, 1 = AVIF, 2 = HEIF
public func capture_screenshot(format: UInt8) -> RustVec<UInt8> {
    let sem = DispatchSemaphore(value: 0)
    var resultData: Data?

    if #available(macOS 12.3, *) {
        SCShareableContent.getExcludingDesktopWindows(false, onScreenWindowsOnly: true) { content, error in
            defer { sem.signal() }

            guard let content = content, let display = content.displays.first else {
                return
            }

            let filter = SCContentFilter(display: display, excludingWindows: [])
            let config = SCStreamConfiguration()
            config.width = Int(display.width)
            config.height = Int(display.height)
            config.pixelFormat = kCVPixelFormatType_32BGRA
            config.showsCursor = true

            // Capture single frame
            if #available(macOS 14.0, *) {
                SCScreenshotManager.captureImage(contentFilter: filter, configuration: config) { image, error in
                    if let cgImage = image {
                        resultData = encodeImage(cgImage, format: format)
                    }
                }
            } else {
                // Fallback for older macOS - use stream with single frame
                resultData = captureWithStream(filter: filter, config: config, format: format)
            }
        }
    } else {
        // Fallback: CGWindowListCreateImage
        if let cgImage = CGWindowListCreateImage(.null, .optionOnScreenOnly, kCGNullWindowID, .bestResolution) {
            resultData = encodeImage(cgImage, format: format)
        }
        sem.signal()
    }

    _ = sem.wait(timeout: .now() + 5.0)

    let vec = RustVec<UInt8>()
    if let data = resultData {
        for byte in data {
            vec.push(value: byte)
        }
    }
    return vec
}

/// Encode CGImage to the specified format
private func encodeImage(_ image: CGImage, format: UInt8) -> Data? {
    let uti: CFString
    switch format {
    case 1: // AVIF
        // AVIF UTI: public.avif (available macOS 13+)
        uti = "public.avif" as CFString
    case 2: // HEIF
        uti = "public.heic" as CFString
    default: // PNG
        uti = "public.png" as CFString
    }

    let data = NSMutableData()
    guard let dest = CGImageDestinationCreateWithData(data, uti, 1, nil) else {
        return nil
    }

    CGImageDestinationAddImage(dest, image, nil)
    if CGImageDestinationFinalize(dest) {
        return data as Data
    }
    return nil
}

/// Capture using SCStream (fallback for macOS < 14)
@available(macOS 12.3, *)
private func captureWithStream(filter: SCContentFilter, config: SCStreamConfiguration, format: UInt8) -> Data? {
    let handler = SingleFrameHandler(format: format)
    let sem = DispatchSemaphore(value: 0)

    do {
        let stream = SCStream(filter: filter, configuration: config, delegate: handler)
        try stream.addStreamOutput(handler, type: .screen, sampleHandlerQueue: .main)
        handler.completion = { sem.signal() }
        stream.startCapture { _ in }
        _ = sem.wait(timeout: .now() + 2.0)
        stream.stopCapture()
        return handler.resultData
    } catch {
        return nil
    }
}

@available(macOS 12.3, *)
private class SingleFrameHandler: NSObject, SCStreamOutput, SCStreamDelegate {
    let format: UInt8
    var resultData: Data?
    var completion: (() -> Void)?

    init(format: UInt8) {
        self.format = format
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, let pixelBuffer = sampleBuffer.imageBuffer else { return }

        let ciImage = CIImage(cvImageBuffer: pixelBuffer)
        let context = CIContext()
        let width = CVPixelBufferGetWidth(pixelBuffer)
        let height = CVPixelBufferGetHeight(pixelBuffer)

        if let cgImage = context.createCGImage(ciImage, from: CGRect(x: 0, y: 0, width: width, height: height)) {
            resultData = encodeImage(cgImage, format: format)
        }
        completion?()
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        completion?()
    }
}

// MARK: - Brightness Control

private func mainDisplayService() -> io_service_t? {
    let displayId = CGMainDisplayID()
    let vendorId = CGDisplayVendorNumber(displayId)
    let productId = CGDisplayModelNumber(displayId)
    let serial = CGDisplaySerialNumber(displayId)

    var iterator: io_iterator_t = 0
    let matching = IOServiceMatching("IODisplayConnect")
    let mainPort: mach_port_t
    if #available(macOS 12.0, *) {
        mainPort = kIOMainPortDefault
    } else {
        mainPort = kIOMasterPortDefault
    }

    let status = IOServiceGetMatchingServices(mainPort, matching, &iterator)
    guard status == KERN_SUCCESS else {
        return nil
    }
    defer { IOObjectRelease(iterator) }

    var service = IOIteratorNext(iterator)
    while service != 0 {
        let info = IODisplayCreateInfoDictionary(
            service,
            IOOptionBits(kIODisplayOnlyPreferredName),
        ).takeRetainedValue() as NSDictionary

        let serviceVendor = (info[kDisplayVendorID as String] as? NSNumber)?.uint32Value ?? 0
        let serviceProduct = (info[kDisplayProductID as String] as? NSNumber)?.uint32Value ?? 0
        let serviceSerial = (info[kDisplaySerialNumber as String] as? NSNumber)?.uint32Value ?? 0

        let serialMatches = serial == 0 || serviceSerial == 0 || serviceSerial == serial
        if serviceVendor == vendorId && serviceProduct == productId && serialMatches {
            return service
        }

        IOObjectRelease(service)
        service = IOIteratorNext(iterator)
    }

    return nil
}

public func get_screen_brightness() -> Float {
    guard let service = mainDisplayService() else {
        return -1.0
    }
    defer { IOObjectRelease(service) }

    var brightness: Float = -1.0
    let result = IODisplayGetFloatParameter(service, 0, kIODisplayBrightnessKey as CFString, &brightness)
    if result != KERN_SUCCESS {
        return -1.0
    }

    return max(0.0, min(1.0, brightness))
}

public func set_screen_brightness(value: Float) -> Bool {
    guard let service = mainDisplayService() else {
        return false
    }
    defer { IOObjectRelease(service) }

    let clamped = max(0.0, min(1.0, value))
    let result = IODisplaySetFloatParameter(service, 0, kIODisplayBrightnessKey as CFString, clamped)
    return result == KERN_SUCCESS
}

// MARK: - High-Performance Stream Capturer

fileprivate var streamCapturer: SCKStreamCapturer? = nil
fileprivate var lastIOSurface: IOSurfaceRef? = nil
fileprivate var ioSurfaceSequence: UInt32 = 0
fileprivate var frameWidth: UInt32 = 0
fileprivate var frameHeight: UInt32 = 0
fileprivate var frameTimestamp: UInt64 = 0
fileprivate let frameLock = NSLock()

@available(macOS 12.3, *)
class SCKStreamCapturer: NSObject, SCStreamOutput, SCStreamDelegate {
    private var stream: SCStream?
    private var isRunning = false

    func start(displayId: UInt32, fps: UInt32, showCursor: Bool, completion: @escaping (Bool) -> Void) {
        guard !isRunning else {
            completion(true)
            return
        }

        SCShareableContent.getExcludingDesktopWindows(false, onScreenWindowsOnly: true) { [weak self] content, error in
            guard let self = self, let content = content, error == nil else {
                completion(false)
                return
            }

            // Find display by ID or use first
            let display = content.displays.first(where: { $0.displayID == CGDirectDisplayID(displayId) }) ?? content.displays.first
            guard let display = display else {
                completion(false)
                return
            }

            let filter = SCContentFilter(display: display, excludingWindows: [])
            let config = SCStreamConfiguration()
            config.width = Int(display.width)
            config.height = Int(display.height)
            config.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(fps))
            config.queueDepth = 8
            config.pixelFormat = kCVPixelFormatType_32BGRA
            config.showsCursor = showCursor
            if #available(macOS 13.0, *) {
                config.capturesAudio = false
            }

            do {
                self.stream = SCStream(filter: filter, configuration: config, delegate: self)
                try self.stream!.addStreamOutput(self, type: .screen, sampleHandlerQueue: DispatchQueue.global(qos: .userInteractive))

                self.stream!.startCapture { error in
                    if error == nil {
                        self.isRunning = true
                    }
                    completion(error == nil)
                }
            } catch {
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
        let ioSurface = CVPixelBufferGetIOSurface(pixelBuffer)?.takeUnretainedValue()

        // Get presentation timestamp
        let pts = CMSampleBufferGetPresentationTimeStamp(sampleBuffer)
        let timestampNs = UInt64(CMTimeGetSeconds(pts) * 1_000_000_000)

        frameLock.lock()
        frameWidth = UInt32(width)
        frameHeight = UInt32(height)
        frameTimestamp = timestampNs

        if let surface = ioSurface {
            lastIOSurface = surface
            ioSurfaceSequence += 1
        }
        frameLock.unlock()
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        isRunning = false
    }
}

/// Initialize screen stream
public func init_screen_stream(display_id: UInt32, target_fps: UInt32, show_cursor: Bool) -> Bool {
    if #available(macOS 12.3, *) {
        let capturer = SCKStreamCapturer()
        var success = false
        let sem = DispatchSemaphore(value: 0)

        capturer.start(displayId: display_id, fps: target_fps, showCursor: show_cursor) { result in
            success = result
            sem.signal()
        }

        _ = sem.wait(timeout: .now() + 3.0)

        if success {
            streamCapturer = capturer
        }
        return success
    }
    return false
}

/// Stop screen stream
public func stop_screen_stream() {
    streamCapturer?.stop()
    streamCapturer = nil

    frameLock.lock()
    lastIOSurface = nil
    ioSurfaceSequence = 0
    frameWidth = 0
    frameHeight = 0
    frameLock.unlock()
}

/// Get IOSurface pointer for zero-copy GPU access
public func get_iosurface_ptr() -> UInt64 {
    frameLock.lock()
    defer { frameLock.unlock() }

    guard let surface = lastIOSurface else { return 0 }
    return UInt64(UInt(bitPattern: Unmanaged.passUnretained(surface as AnyObject).toOpaque()))
}

/// Get IOSurface sequence number
public func get_iosurface_sequence() -> UInt32 {
    frameLock.lock()
    defer { frameLock.unlock() }
    return ioSurfaceSequence
}

/// Get frame width
public func get_frame_width() -> UInt32 {
    frameLock.lock()
    defer { frameLock.unlock() }
    return frameWidth
}

/// Get frame height
public func get_frame_height() -> UInt32 {
    frameLock.lock()
    defer { frameLock.unlock() }
    return frameHeight
}

/// Get frame timestamp in nanoseconds
public func get_frame_timestamp_ns() -> UInt64 {
    frameLock.lock()
    defer { frameLock.unlock() }
    return frameTimestamp
}
