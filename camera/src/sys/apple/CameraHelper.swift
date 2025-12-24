import Foundation
import AVFoundation
import CoreMedia
import CoreVideo
import Metal

// MARK: - Camera State

private var captureSession: AVCaptureSession?
private var videoOutput: AVCaptureVideoDataOutput?
private var currentDevice: AVCaptureDevice?
private var cachedDevices: [AVCaptureDevice] = []

// Frame data - keep CVPixelBuffer for IOSurface access
private var latestPixelBuffer: CVPixelBuffer?
private var latestFrameWidth: UInt32 = 0
private var latestFrameHeight: UInt32 = 0
private var latestFrameFormat: UInt8 = 2 // BGRA
private let frameQueue = DispatchQueue(label: "waterkit.camera.frame", qos: .userInteractive)
private let frameLock = NSLock()

// MARK: - Frame Delegate

class CameraFrameDelegate: NSObject, AVCaptureVideoDataOutputSampleBufferDelegate {
    func captureOutput(_ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer, from connection: AVCaptureConnection) {
        guard let pixelBuffer = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }
        
        let width = CVPixelBufferGetWidth(pixelBuffer)
        let height = CVPixelBufferGetHeight(pixelBuffer)
        
        frameLock.lock()
        // ARC retains pixelBuffer when assigned to optional property
        latestPixelBuffer = pixelBuffer
        latestFrameWidth = UInt32(width)
        latestFrameHeight = UInt32(height)
        latestFrameFormat = 2 // BGRA
        frameLock.unlock()
    }

    func captureOutput(_ output: AVCaptureOutput, didDrop sampleBuffer: CMSampleBuffer, from connection: AVCaptureConnection) {
        droppedFrameCount.increment()
    }
}

// Thread-safe counter
class AtomicUInt64 {
    private var val: UInt64 = 0
    private let lock = NSLock()
    
    func increment() {
        lock.lock()
        val += 1
        lock.unlock()
    }
    
    func get() -> UInt64 {
        lock.lock()
        let v = val
        lock.unlock()
        return v
    }
}

private let droppedFrameCount = AtomicUInt64()

private var frameDelegate = CameraFrameDelegate()

// MARK: - Device Enumeration

func camera_device_count() -> Int32 {
    #if os(iOS)
    let deviceTypes: [AVCaptureDevice.DeviceType] = [.builtInWideAngleCamera, .builtInTelephotoCamera, .builtInUltraWideCamera]
    #else
    let deviceTypes: [AVCaptureDevice.DeviceType] = [.builtInWideAngleCamera, .externalUnknown]
    #endif
    
    let discoverySession = AVCaptureDevice.DiscoverySession(
        deviceTypes: deviceTypes,
        mediaType: .video,
        position: .unspecified
    )
    
    cachedDevices = discoverySession.devices
    return Int32(cachedDevices.count)
}

func camera_device_id(index: Int32) -> RustString {
    guard index >= 0 && index < cachedDevices.count else {
        return RustString()
    }
    return cachedDevices[Int(index)].uniqueID.intoRustString()
}

func camera_device_name(index: Int32) -> RustString {
    guard index >= 0 && index < cachedDevices.count else {
        return RustString()
    }
    return cachedDevices[Int(index)].localizedName.intoRustString()
}

func camera_device_description(index: Int32) -> RustString {
    guard index >= 0 && index < cachedDevices.count else {
        return RustString()
    }
    return cachedDevices[Int(index)].modelID.intoRustString()
}

func camera_device_is_front(index: Int32) -> Bool {
    guard index >= 0 && index < cachedDevices.count else {
        return false
    }
    return cachedDevices[Int(index)].position == .front
}

// MARK: - Camera Control

func camera_open(device_id: RustString) -> CameraResultFFI {
    let deviceId = device_id.toString()
    
    #if os(iOS)
    let deviceTypes: [AVCaptureDevice.DeviceType] = [.builtInWideAngleCamera, .builtInTelephotoCamera, .builtInUltraWideCamera]
    #else
    let deviceTypes: [AVCaptureDevice.DeviceType] = [.builtInWideAngleCamera, .externalUnknown]
    #endif
    
    let discoverySession = AVCaptureDevice.DiscoverySession(
        deviceTypes: deviceTypes,
        mediaType: .video,
        position: .unspecified
    )
    
    guard let device = discoverySession.devices.first(where: { $0.uniqueID == deviceId }) else {
        return .NotFound
    }
    
    let session = AVCaptureSession()
    session.sessionPreset = .high
    
    do {
        let input = try AVCaptureDeviceInput(device: device)
        if session.canAddInput(input) {
            session.addInput(input)
        } else {
            return .OpenFailed
        }
    } catch {
        return .OpenFailed
    }
    
    let output = AVCaptureVideoDataOutput()
    // Use BGRA format with IOSurface backing for Metal compatibility
    output.videoSettings = [
        kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
        kCVPixelBufferMetalCompatibilityKey as String: true
    ]
    output.setSampleBufferDelegate(frameDelegate, queue: frameQueue)
    output.alwaysDiscardsLateVideoFrames = true
    
    if session.canAddOutput(output) {
        session.addOutput(output)
    } else {
        return .OpenFailed
    }
    
    captureSession = session
    videoOutput = output
    currentDevice = device

    // Enable HDR by default if supported (iOS only)
    #if os(iOS)
    if device.activeFormat.isVideoHDRSupported {
        do {
            try device.lockForConfiguration()
            device.automaticallyAdjustsVideoHDREnabled = false
            device.isVideoHDREnabled = true
            device.unlockForConfiguration()
        } catch {
            print("Failed to enable HDR: \(error)")
        }
    }
    #endif

    return .Success
}

func camera_start() -> CameraResultFFI {
    guard let session = captureSession else {
        return .StartFailed
    }
    
    if !session.isRunning {
        session.startRunning()
    }
    
    return .Success
}

func camera_stop() -> CameraResultFFI {
    guard let session = captureSession else {
        return .Success
    }
    
    if session.isRunning {
        session.stopRunning()
    }
    
    return .Success
}

// MARK: - Frame Access (Zero-Copy via IOSurface)

func camera_has_frame() -> Bool {
    frameLock.lock()
    let hasFrame = latestPixelBuffer != nil
    frameLock.unlock()
    return hasFrame
}

func camera_frame_width() -> UInt32 {
    frameLock.lock()
    let width = latestFrameWidth
    frameLock.unlock()
    return width
}

func camera_frame_height() -> UInt32 {
    frameLock.lock()
    let height = latestFrameHeight
    frameLock.unlock()
    return height
}

func camera_frame_format() -> UInt8 {
    frameLock.lock()
    let format = latestFrameFormat
    frameLock.unlock()
    return format
}

func camera_get_dropped_frame_count() -> UInt64 {
    return droppedFrameCount.get()
}

/// Get IOSurface pointer for zero-copy Metal texture creation.
/// Returns the IOSurfaceRef as a u64 pointer value, or 0 if not available.
func camera_get_iosurface() -> UInt64 {
    frameLock.lock()
    defer { frameLock.unlock() }
    
    guard let pixelBuffer = latestPixelBuffer else {
        return 0
    }
    
    // Get the IOSurface backing the pixel buffer
    guard let ioSurface = CVPixelBufferGetIOSurface(pixelBuffer) else {
        return 0
    }
    
    // Return a retained pointer so Rust can manage lifecycle
    // Cast to AnyObject to satisfy Unmanaged requirements
    let unmanaged = Unmanaged.passRetained(ioSurface as AnyObject)
    return UInt64(UInt(bitPattern: unmanaged.toOpaque()))
}

@_cdecl("camera_retain_iosurface")
public func camera_retain_iosurface(handle: UInt64) {
    if handle == 0 { return }
    guard let ptr = UnsafeRawPointer(bitPattern: UInt(handle)) else { return }
    let unmanaged = Unmanaged<AnyObject>.fromOpaque(ptr)
    _ = unmanaged.retain()
}

@_cdecl("camera_release_iosurface")
public func camera_release_iosurface(handle: UInt64) {
    if handle == 0 { return }
    guard let ptr = UnsafeRawPointer(bitPattern: UInt(handle)) else { return }
    let unmanaged = Unmanaged<AnyObject>.fromOpaque(ptr)
    unmanaged.release()
}

@_cdecl("camera_copy_frame_data")
public func camera_copy_frame_data(_ bufferPtr: UInt64, _ size: UInt64) {
    frameLock.lock()
    defer { frameLock.unlock() }
    guard let pixelBuffer = latestPixelBuffer else { return }
    
    // Convert u64 back to pointer
    guard let buffer = UnsafeMutableRawPointer(bitPattern: UInt(bufferPtr)) else { return }
    
    CVPixelBufferLockBaseAddress(pixelBuffer, .readOnly)
    defer { CVPixelBufferUnlockBaseAddress(pixelBuffer, .readOnly) }
    
    if let baseAddress = CVPixelBufferGetBaseAddress(pixelBuffer) {
        let height = Int(CVPixelBufferGetHeight(pixelBuffer))
        let width = Int(CVPixelBufferGetWidth(pixelBuffer))
        let bytesPerRow = CVPixelBufferGetBytesPerRow(pixelBuffer)
        let rowBytes = width * 4
        
        // Copy row by row to handle stride differences and ensure tight packing
        for y in 0..<height {
             let src = baseAddress.advanced(by: y * bytesPerRow)
             let dst = buffer.advanced(by: y * rowBytes)
             // Safety: Ensure we don't write past end of buffer
             if (y * rowBytes) + rowBytes <= Int(size) {
                 dst.copyMemory(from: src, byteCount: rowBytes)
             }
        }
    }
}

func camera_consume_frame() {
    frameLock.lock()
    // ARC handles release of old buffer when setting to nil
    latestPixelBuffer = nil
    frameLock.unlock()
}

// MARK: - Resolution

func camera_set_resolution(width: UInt32, height: UInt32) -> CameraResultFFI {
    guard let session = captureSession else {
        return .OpenFailed
    }
    
    let presets: [(AVCaptureSession.Preset, Int, Int)] = [
        (.hd4K3840x2160, 3840, 2160),
        (.hd1920x1080, 1920, 1080),
        (.hd1280x720, 1280, 720),
        (.vga640x480, 640, 480),
        (.cif352x288, 352, 288),
    ]
    
    var bestPreset = AVCaptureSession.Preset.high
    var bestDiff = Int.max
    
    for (preset, w, h) in presets {
        let diff = abs(Int(width) - w) + abs(Int(height) - h)
        if diff < bestDiff && session.canSetSessionPreset(preset) {
            bestDiff = diff
            bestPreset = preset
        }
    }
    
    session.beginConfiguration()
    session.sessionPreset = bestPreset
    session.commitConfiguration()
    
    return .Success
}

func camera_get_resolution_width() -> UInt32 {
    guard let session = captureSession else { return 1280 }
    
    switch session.sessionPreset {
    case .hd4K3840x2160: return 3840
    case .hd1920x1080: return 1920
    case .hd1280x720: return 1280
    case .vga640x480: return 640
    case .cif352x288: return 352
    default: return 1280
    }
}

func camera_get_resolution_height() -> UInt32 {
    guard let session = captureSession else { return 720 }
    
    switch session.sessionPreset {
    case .hd4K3840x2160: return 2160
    case .hd1920x1080: return 1080
    case .hd1280x720: return 720
    case .vga640x480: return 480
    case .cif352x288: return 288
    default: return 720
    }
}

// MARK: - HDR Control

func camera_set_hdr(enabled: Bool) -> CameraResultFFI {
    #if os(iOS)
    guard let device = currentDevice else {
        return .OpenFailed
    }
    
    if !device.activeFormat.isVideoHDRSupported {
        return .NotSupported
    }
    
    do {
        try device.lockForConfiguration()
        device.automaticallyAdjustsVideoHDREnabled = false
        device.isVideoHDREnabled = enabled
        device.unlockForConfiguration()
        return .Success
    } catch {
        return .OpenFailed
    }
    #else
    return .NotSupported
    #endif
}

func camera_get_hdr() -> Bool {
    #if os(iOS)
    guard let device = currentDevice else {
        return false
    }
    return device.isVideoHDREnabled
    #else
    return false
    #endif
}
