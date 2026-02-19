import Foundation
import AVFoundation
import CoreMedia
import CoreVideo
import Metal

// MARK: - Camera State

private var captureSession: AVCaptureSession?
private var videoOutput: AVCaptureVideoDataOutput?
private var photoOutput: AVCapturePhotoOutput?
private var movieOutput: AVCaptureMovieFileOutput?
private var currentDevice: AVCaptureDevice?
private var cachedDevices: [AVCaptureDevice] = []
private var recordingStartTime: Date?

// Photo capture state
private var lastPhotoData: Data?
private let photoLock = NSLock()
private var lastRawPhotoData: Data?
private let rawPhotoLock = NSLock()

// RAW video frame stream state
private var rawVideoFileHandle: FileHandle?
private var rawVideoRecordingStartTime: Date?
private let rawVideoLock = NSLock()

// Frame callback - set from Rust
private var frameCallback: ((UInt64, UInt32, UInt32, UInt64) -> Void)?
private let frameQueue = DispatchQueue(label: "waterkit.camera.frame", qos: .userInteractive)
private let frameLock = NSLock()

// Capabilities cache
private var cachedIsoMin: Float = 0
private var cachedIsoMax: Float = 0
private var cachedExposureDurationMinNs: UInt64 = 0
private var cachedExposureDurationMaxNs: UInt64 = 0
private var cachedSupportsExposureCompensation: Bool = false
private var cachedSupportsManualFocus: Bool = false
private var cachedSupportsManualWhiteBalance: Bool = false
private var cachedZoomMin: Float = 1.0
private var cachedZoomMax: Float = 1.0
private var cachedSupportsHdr: Bool = false
private var cachedSupportsDolbyVision: Bool = false
private var cachedSupportsStandardStabilization: Bool = false
private var cachedSupportsCinematicStabilization: Bool = false
private var cachedHasFlash: Bool = false
private var cachedHasTorch: Bool = false
private var cachedSupportsConcurrentMultiCam: Bool = false
private var cachedMaxConcurrentCameras: UInt8 = 1
private var cachedSupportsRawPhoto: Bool = false
private var cachedSupportsRawVideo: Bool = true

// MARK: - Frame Delegate

class CameraFrameDelegate: NSObject, AVCaptureVideoDataOutputSampleBufferDelegate {
    func captureOutput(_ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer, from connection: AVCaptureConnection) {
        guard let pixelBuffer = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }

        let width = UInt32(CVPixelBufferGetWidth(pixelBuffer))
        let height = UInt32(CVPixelBufferGetHeight(pixelBuffer))

        // Get presentation timestamp
        let pts = CMSampleBufferGetPresentationTimeStamp(sampleBuffer)
        let timestampNs = UInt64(CMTimeGetSeconds(pts) * 1_000_000_000)

        // Retain the CVPixelBuffer and pass to Rust callback
        let unmanaged = Unmanaged.passRetained(pixelBuffer)
        let handle = UInt64(UInt(bitPattern: unmanaged.toOpaque()))

        frameLock.lock()
        let callback = frameCallback
        frameLock.unlock()

        callback?(handle, width, height, timestampNs)
        maybeWriteRawVideoFrame(pixelBuffer: pixelBuffer, timestampNs: timestampNs)
    }

    func captureOutput(_ output: AVCaptureOutput, didDrop sampleBuffer: CMSampleBuffer, from connection: AVCaptureConnection) {
        // Frame dropped - backpressure from consumer
    }
}

private var frameDelegate = CameraFrameDelegate()

private func appendUInt16LE(_ value: UInt16, to data: inout Data) {
    var little = value.littleEndian
    withUnsafeBytes(of: &little) { data.append(contentsOf: $0) }
}

private func appendUInt32LE(_ value: UInt32, to data: inout Data) {
    var little = value.littleEndian
    withUnsafeBytes(of: &little) { data.append(contentsOf: $0) }
}

private func appendUInt64LE(_ value: UInt64, to data: inout Data) {
    var little = value.littleEndian
    withUnsafeBytes(of: &little) { data.append(contentsOf: $0) }
}

private func writeRawVideoHeader(handle: FileHandle, width: UInt32, height: UInt32) {
    var header = Data()
    header.append(contentsOf: [UInt8(ascii: "W"), UInt8(ascii: "K"), UInt8(ascii: "R"), UInt8(ascii: "V")])
    header.append(contentsOf: [1, 1]) // version=1, pixel_format=1(BGRA8)
    appendUInt16LE(0, to: &header)
    appendUInt32LE(width, to: &header)
    appendUInt32LE(height, to: &header)
    appendUInt32LE(0, to: &header) // fps unknown from this layer
    handle.write(header)
}

private func maybeWriteRawVideoFrame(pixelBuffer: CVPixelBuffer, timestampNs: UInt64) {
    rawVideoLock.lock()
    let handle = rawVideoFileHandle
    rawVideoLock.unlock()

    guard let handle else { return }

    let lockResult = CVPixelBufferLockBaseAddress(pixelBuffer, .readOnly)
    if lockResult != kCVReturnSuccess {
        return
    }
    defer { CVPixelBufferUnlockBaseAddress(pixelBuffer, .readOnly) }

    guard let baseAddress = CVPixelBufferGetBaseAddress(pixelBuffer) else { return }
    let width = CVPixelBufferGetWidth(pixelBuffer)
    let height = CVPixelBufferGetHeight(pixelBuffer)
    let bytesPerRow = CVPixelBufferGetBytesPerRow(pixelBuffer)
    let rowBytes = width * 4
    let payloadSize = rowBytes * height

    var frameHeader = Data()
    appendUInt64LE(timestampNs, to: &frameHeader)
    appendUInt32LE(UInt32(payloadSize), to: &frameHeader)
    handle.write(frameHeader)

    for row in 0..<height {
        let rowPtr = baseAddress.advanced(by: row * bytesPerRow)
        let rowData = Data(bytes: rowPtr, count: rowBytes)
        handle.write(rowData)
    }
}

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
    // Use BGRA format
    output.videoSettings = [
        kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA
    ]
    output.setSampleBufferDelegate(frameDelegate, queue: frameQueue)
    output.alwaysDiscardsLateVideoFrames = true

    if session.canAddOutput(output) {
        session.addOutput(output)
    } else {
        return .OpenFailed
    }

    // Add Photo Output
    let pOutput = AVCapturePhotoOutput()
    if session.canAddOutput(pOutput) {
        session.addOutput(pOutput)
        #if os(iOS)
        pOutput.isHighResolutionCaptureEnabled = true
        #endif
    }

    // Add Movie File Output
    let mOutput = AVCaptureMovieFileOutput()
    if session.canAddOutput(mOutput) {
        session.addOutput(mOutput)
    }

    captureSession = session
    videoOutput = output
    photoOutput = pOutput
    movieOutput = mOutput
    currentDevice = device

    // Cache capabilities
    queryCapabilities(device: device, movieOutput: mOutput)

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

    _ = camera_stop_raw_recording()

    return .Success
}

func camera_is_streaming() -> Bool {
    return captureSession?.isRunning ?? false
}

// MARK: - Frame Callback

@_cdecl("camera_set_frame_callback")
public func camera_set_frame_callback(callback: @escaping @convention(c) (UInt64, UInt32, UInt32, UInt64) -> Void) {
    frameLock.lock()
    frameCallback = callback
    frameLock.unlock()
}

@_cdecl("camera_clear_frame_callback")
public func camera_clear_frame_callback() {
    frameLock.lock()
    frameCallback = nil
    frameLock.unlock()
}

@_cdecl("camera_release_pixelbuffer")
public func camera_release_pixelbuffer(handle: UInt64) {
    if handle == 0 { return }
    guard let ptr = UnsafeRawPointer(bitPattern: UInt(handle)) else { return }
    let unmanaged = Unmanaged<CVPixelBuffer>.fromOpaque(ptr)
    unmanaged.release()
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

// MARK: - Capabilities Query

private func queryCapabilities(device: AVCaptureDevice, movieOutput: AVCaptureMovieFileOutput?) {
    #if os(iOS)
    cachedSupportsRawPhoto = photoOutput?.availableRawPhotoPixelFormatTypes.isEmpty == false
    #else
    cachedSupportsRawPhoto = false
    #endif
    cachedSupportsRawVideo = true
    #if os(iOS)
    let format = device.activeFormat

    // ISO range (iOS only)
    cachedIsoMin = format.minISO
    cachedIsoMax = format.maxISO

    // Exposure duration range (iOS only)
    let minDuration = format.minExposureDuration
    let maxDuration = format.maxExposureDuration
    cachedExposureDurationMinNs = UInt64(CMTimeGetSeconds(minDuration) * 1_000_000_000)
    cachedExposureDurationMaxNs = UInt64(CMTimeGetSeconds(maxDuration) * 1_000_000_000)

    // HDR (iOS only)
    cachedSupportsHdr = format.isVideoHDRSupported
    cachedSupportsDolbyVision =
        cachedSupportsHdr && (movieOutput?.availableVideoCodecTypes.contains(.hevc) ?? false)

    // Stabilization (iOS only)
    cachedSupportsStandardStabilization = format.isVideoStabilizationModeSupported(.standard)
    cachedSupportsCinematicStabilization = format.isVideoStabilizationModeSupported(.cinematic)

    // Exposure compensation (iOS only)
    cachedSupportsExposureCompensation = device.minExposureTargetBias != device.maxExposureTargetBias

    // Zoom (iOS only)
    cachedZoomMin = Float(device.minAvailableVideoZoomFactor)
    cachedZoomMax = Float(device.maxAvailableVideoZoomFactor)
    if #available(iOS 13.0, *) {
        cachedSupportsConcurrentMultiCam = AVCaptureMultiCamSession.isMultiCamSupported
    } else {
        cachedSupportsConcurrentMultiCam = false
    }
    cachedMaxConcurrentCameras = cachedSupportsConcurrentMultiCam ? 2 : 1
    #else
    // macOS doesn't support these features
    cachedIsoMin = 0
    cachedIsoMax = 0
    cachedExposureDurationMinNs = 0
    cachedExposureDurationMaxNs = 0
    cachedSupportsHdr = false
    cachedSupportsDolbyVision = false
    cachedSupportsStandardStabilization = false
    cachedSupportsCinematicStabilization = false
    cachedSupportsExposureCompensation = false
    cachedZoomMin = 1.0
    cachedZoomMax = 1.0
    cachedSupportsConcurrentMultiCam = false
    cachedMaxConcurrentCameras = 1
    #endif

    // Focus (both platforms)
    cachedSupportsManualFocus = device.isFocusModeSupported(.locked)

    // White balance (both platforms)
    cachedSupportsManualWhiteBalance = device.isWhiteBalanceModeSupported(.locked)

    // Flash/Torch (both platforms)
    cachedHasFlash = device.hasFlash
    cachedHasTorch = device.hasTorch
}

// Expose capabilities to Rust
func camera_get_iso_min() -> Float {
    return cachedIsoMin
}

func camera_get_iso_max() -> Float {
    return cachedIsoMax
}

func camera_get_exposure_duration_min_ns() -> UInt64 {
    return cachedExposureDurationMinNs
}

func camera_get_exposure_duration_max_ns() -> UInt64 {
    return cachedExposureDurationMaxNs
}

func camera_supports_exposure_compensation() -> Bool {
    return cachedSupportsExposureCompensation
}

func camera_supports_manual_focus() -> Bool {
    return cachedSupportsManualFocus
}

func camera_supports_manual_white_balance() -> Bool {
    return cachedSupportsManualWhiteBalance
}

func camera_get_zoom_min() -> Float {
    return cachedZoomMin
}

func camera_get_zoom_max() -> Float {
    return cachedZoomMax
}

func camera_supports_hdr() -> Bool {
    return cachedSupportsHdr
}

func camera_supports_dolby_vision() -> Bool {
    return cachedSupportsDolbyVision
}

func camera_supports_standard_stabilization() -> Bool {
    return cachedSupportsStandardStabilization
}

func camera_supports_cinematic_stabilization() -> Bool {
    return cachedSupportsCinematicStabilization
}

func camera_has_flash() -> Bool {
    return cachedHasFlash
}

func camera_has_torch() -> Bool {
    return cachedHasTorch
}

func camera_supports_concurrent_multicam() -> Bool {
    return cachedSupportsConcurrentMultiCam
}

func camera_max_concurrent_cameras() -> UInt8 {
    return cachedMaxConcurrentCameras
}

func camera_supports_raw_photo() -> Bool {
    return cachedSupportsRawPhoto
}

func camera_supports_raw_video() -> Bool {
    return cachedSupportsRawVideo
}

// MARK: - Exposure Control

func camera_set_exposure_mode(mode: UInt8) -> CameraResultFFI {
    #if os(iOS)
    guard let device = currentDevice else { return .OpenFailed }

    let avMode: AVCaptureDevice.ExposureMode
    switch mode {
    case 0: avMode = .continuousAutoExposure
    case 1: avMode = .custom
    case 2: avMode = .locked
    default: return .NotSupported
    }

    guard device.isExposureModeSupported(avMode) else { return .NotSupported }

    do {
        try device.lockForConfiguration()
        device.exposureMode = avMode
        device.unlockForConfiguration()
        return .Success
    } catch {
        return .OpenFailed
    }
    #else
    return .NotSupported
    #endif
}

func camera_set_iso(iso: Float) -> CameraResultFFI {
    #if os(iOS)
    guard let device = currentDevice else { return .OpenFailed }

    let format = device.activeFormat
    let clampedIso = max(format.minISO, min(format.maxISO, iso))

    do {
        try device.lockForConfiguration()
        device.setExposureModeCustom(duration: AVCaptureDevice.currentExposureDuration, iso: clampedIso)
        device.unlockForConfiguration()
        return .Success
    } catch {
        return .OpenFailed
    }
    #else
    return .NotSupported
    #endif
}

func camera_set_exposure_duration_ns(duration_ns: UInt64) -> CameraResultFFI {
    #if os(iOS)
    guard let device = currentDevice else { return .OpenFailed }

    let duration = CMTime(value: CMTimeValue(duration_ns), timescale: 1_000_000_000)
    let format = device.activeFormat

    // Clamp to valid range
    var clampedDuration = duration
    if CMTimeCompare(duration, format.minExposureDuration) < 0 {
        clampedDuration = format.minExposureDuration
    } else if CMTimeCompare(duration, format.maxExposureDuration) > 0 {
        clampedDuration = format.maxExposureDuration
    }

    do {
        try device.lockForConfiguration()
        device.setExposureModeCustom(duration: clampedDuration, iso: AVCaptureDevice.currentISO)
        device.unlockForConfiguration()
        return .Success
    } catch {
        return .OpenFailed
    }
    #else
    return .NotSupported
    #endif
}

func camera_set_exposure_compensation(ev: Float) -> CameraResultFFI {
    #if os(iOS)
    guard let device = currentDevice else { return .OpenFailed }

    let clampedEv = max(device.minExposureTargetBias, min(device.maxExposureTargetBias, ev))

    do {
        try device.lockForConfiguration()
        device.setExposureTargetBias(clampedEv)
        device.unlockForConfiguration()
        return .Success
    } catch {
        return .OpenFailed
    }
    #else
    return .NotSupported
    #endif
}

// MARK: - Focus Control

func camera_set_focus_mode(mode: UInt8) -> CameraResultFFI {
    guard let device = currentDevice else { return .OpenFailed }

    let avMode: AVCaptureDevice.FocusMode
    switch mode {
    case 0: avMode = .continuousAutoFocus
    case 1: avMode = .autoFocus
    case 2: avMode = .locked
    case 3: avMode = .locked
    default: return .NotSupported
    }

    guard device.isFocusModeSupported(avMode) else { return .NotSupported }

    do {
        try device.lockForConfiguration()
        device.focusMode = avMode
        device.unlockForConfiguration()
        return .Success
    } catch {
        return .OpenFailed
    }
}

func camera_set_focus_distance(distance: Float) -> CameraResultFFI {
    #if os(iOS)
    guard let device = currentDevice else { return .OpenFailed }
    guard device.isLockingFocusWithCustomLensPositionSupported else { return .NotSupported }

    let clampedDistance = max(0.0, min(1.0, distance))

    do {
        try device.lockForConfiguration()
        device.setFocusModeLocked(lensPosition: clampedDistance)
        device.unlockForConfiguration()
        return .Success
    } catch {
        return .OpenFailed
    }
    #else
    return .NotSupported
    #endif
}

func camera_set_focus_point(x: Float, y: Float) -> CameraResultFFI {
    guard let device = currentDevice else { return .OpenFailed }
    guard device.isFocusPointOfInterestSupported else { return .NotSupported }

    let point = CGPoint(x: CGFloat(x), y: CGFloat(y))

    do {
        try device.lockForConfiguration()
        device.focusPointOfInterest = point
        device.focusMode = .autoFocus
        device.unlockForConfiguration()
        return .Success
    } catch {
        return .OpenFailed
    }
}

// MARK: - White Balance Control

func camera_set_white_balance_mode(mode: UInt8) -> CameraResultFFI {
    guard let device = currentDevice else { return .OpenFailed }

    let avMode: AVCaptureDevice.WhiteBalanceMode
    switch mode {
    case 0: avMode = .continuousAutoWhiteBalance
    case 1: avMode = .locked
    default: avMode = .continuousAutoWhiteBalance
    }

    guard device.isWhiteBalanceModeSupported(avMode) else { return .NotSupported }

    do {
        try device.lockForConfiguration()
        device.whiteBalanceMode = avMode
        device.unlockForConfiguration()
        return .Success
    } catch {
        return .OpenFailed
    }
}

func camera_set_white_balance_temperature(kelvin: UInt32) -> CameraResultFFI {
    #if os(iOS)
    guard let device = currentDevice else { return .OpenFailed }

    let temperatureAndTint = AVCaptureDevice.WhiteBalanceTemperatureAndTintValues(
        temperature: Float(kelvin),
        tint: 0.0
    )

    var gains = device.deviceWhiteBalanceGains(for: temperatureAndTint)

    // Clamp gains to valid range
    let maxGain = device.maxWhiteBalanceGain
    gains.redGain = max(1.0, min(maxGain, gains.redGain))
    gains.greenGain = max(1.0, min(maxGain, gains.greenGain))
    gains.blueGain = max(1.0, min(maxGain, gains.blueGain))

    do {
        try device.lockForConfiguration()
        device.setWhiteBalanceModeLocked(with: gains)
        device.unlockForConfiguration()
        return .Success
    } catch {
        return .OpenFailed
    }
    #else
    return .NotSupported
    #endif
}

// MARK: - Zoom Control

func camera_set_zoom(factor: Float) -> CameraResultFFI {
    #if os(iOS)
    guard let device = currentDevice else { return .OpenFailed }

    let clampedFactor = max(Float(device.minAvailableVideoZoomFactor),
                           min(Float(device.maxAvailableVideoZoomFactor), factor))

    do {
        try device.lockForConfiguration()
        device.videoZoomFactor = CGFloat(clampedFactor)
        device.unlockForConfiguration()
        return .Success
    } catch {
        return .OpenFailed
    }
    #else
    // macOS doesn't support videoZoomFactor
    return .NotSupported
    #endif
}

func camera_get_zoom() -> Float {
    #if os(iOS)
    guard let device = currentDevice else { return 1.0 }
    return Float(device.videoZoomFactor)
    #else
    return 1.0
    #endif
}

// MARK: - Flash/Torch Control

func camera_set_flash_mode(mode: UInt8) -> CameraResultFFI {
    // Flash mode is set during photo capture, not on device
    return .Success
}

func camera_set_torch_mode(enabled: Bool) -> CameraResultFFI {
    guard let device = currentDevice else { return .OpenFailed }
    guard device.hasTorch else { return .NotSupported }

    do {
        try device.lockForConfiguration()
        device.torchMode = enabled ? .on : .off
        device.unlockForConfiguration()
        return .Success
    } catch {
        return .OpenFailed
    }
}

// MARK: - HDR Control

func camera_set_hdr(enabled: Bool) -> CameraResultFFI {
    return camera_set_dynamic_range(profile: enabled ? 1 : 0)
}

func camera_set_dynamic_range(profile: UInt8) -> CameraResultFFI {
    #if os(iOS)
    guard let device = currentDevice else {
        return .OpenFailed
    }

    if profile == 3 && !cachedSupportsDolbyVision {
        return .NotSupported
    }

    let enableHdr: Bool
    switch profile {
    case 0:
        enableHdr = false
    case 1, 2, 3:
        if !device.activeFormat.isVideoHDRSupported {
            return .NotSupported
        }
        enableHdr = true
    default:
        return .NotSupported
    }

    do {
        try device.lockForConfiguration()
        device.automaticallyAdjustsVideoHDREnabled = false
        device.isVideoHDREnabled = enableHdr
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

// MARK: - Stabilization Control

func camera_set_stabilization_mode(mode: UInt8) -> CameraResultFFI {
    #if os(iOS)
    guard let connection = videoOutput?.connection(with: .video) else { return .OpenFailed }

    let avMode: AVCaptureVideoStabilizationMode
    switch mode {
    case 0: avMode = .off
    case 1: avMode = .standard
    case 2: avMode = .cinematic
    default: return .NotSupported
    }

    guard connection.isVideoStabilizationSupported else { return .NotSupported }
    connection.preferredVideoStabilizationMode = avMode

    return .Success
    #else
    return .NotSupported
    #endif
}

// MARK: - Photo Capture

class PhotoCaptureDelegate: NSObject, AVCapturePhotoCaptureDelegate {
    let semaphore = DispatchSemaphore(value: 0)
    var photoData: Data?
    var error: Error?

    func photoOutput(_ output: AVCapturePhotoOutput, didFinishProcessingPhoto photo: AVCapturePhoto, error: Error?) {
        if let error = error {
            self.error = error
        } else {
            self.photoData = photo.fileDataRepresentation()
        }
        semaphore.signal()
    }
}

func camera_take_photo() -> CameraResultFFI {
    guard let output = photoOutput else {
        return .NotSupported
    }

    let settings: AVCapturePhotoSettings
    if output.availablePhotoCodecTypes.contains(.jpeg) {
        settings = AVCapturePhotoSettings(format: [AVVideoCodecKey: AVVideoCodecType.jpeg])
    } else {
        settings = AVCapturePhotoSettings()
    }
    #if os(iOS)
    settings.isHighResolutionPhotoEnabled = true
    #endif

    let delegate = PhotoCaptureDelegate()
    output.capturePhoto(with: settings, delegate: delegate)

    // Wait for capture to complete
    let result = delegate.semaphore.wait(timeout: .now() + 10.0)
    if result == .timedOut {
        return .CaptureFailed
    }

    if delegate.error != nil {
        return .CaptureFailed
    }

    guard let data = delegate.photoData else {
        return .CaptureFailed
    }

    photoLock.lock()
    lastPhotoData = data
    photoLock.unlock()

    return .Success
}

func camera_get_photo_len() -> Int32 {
    photoLock.lock()
    let len = lastPhotoData?.count ?? 0
    photoLock.unlock()
    return Int32(len)
}

func camera_take_raw_photo() -> CameraResultFFI {
    #if os(iOS)
    guard let output = photoOutput else {
        return .NotSupported
    }
    guard let rawFormat = output.availableRawPhotoPixelFormatTypes.first else {
        return .NotSupported
    }

    let settings = AVCapturePhotoSettings(rawPixelFormatType: rawFormat)
    #if os(iOS)
    settings.isHighResolutionPhotoEnabled = true
    #endif

    let delegate = PhotoCaptureDelegate()
    output.capturePhoto(with: settings, delegate: delegate)

    let result = delegate.semaphore.wait(timeout: .now() + 10.0)
    if result == .timedOut {
        return .CaptureFailed
    }
    if delegate.error != nil {
        return .CaptureFailed
    }
    guard let data = delegate.photoData else {
        return .CaptureFailed
    }

    rawPhotoLock.lock()
    lastRawPhotoData = data
    rawPhotoLock.unlock()
    return .Success
    #else
    return .NotSupported
    #endif
}

func camera_get_raw_photo_len() -> Int32 {
    rawPhotoLock.lock()
    let len = lastRawPhotoData?.count ?? 0
    rawPhotoLock.unlock()
    return Int32(len)
}

@_cdecl("camera_copy_photo_data")
public func camera_copy_photo_data(_ bufferPtr: UInt64, _ size: UInt64) {
    photoLock.lock()
    defer { photoLock.unlock() }

    guard let data = lastPhotoData else { return }
    guard let buffer = UnsafeMutableRawPointer(bitPattern: UInt(bufferPtr)) else { return }

    let count = min(Int(size), data.count)
    data.copyBytes(to: buffer.assumingMemoryBound(to: UInt8.self), count: count)
}

@_cdecl("camera_copy_raw_photo_data")
public func camera_copy_raw_photo_data(_ bufferPtr: UInt64, _ size: UInt64) {
    rawPhotoLock.lock()
    defer { rawPhotoLock.unlock() }

    guard let data = lastRawPhotoData else { return }
    guard let buffer = UnsafeMutableRawPointer(bitPattern: UInt(bufferPtr)) else { return }

    let count = min(Int(size), data.count)
    data.copyBytes(to: buffer.assumingMemoryBound(to: UInt8.self), count: count)
}

// MARK: - Video Recording

class MovieRecordingDelegate: NSObject, AVCaptureFileOutputRecordingDelegate {
    func fileOutput(_ output: AVCaptureFileOutput, didFinishRecordingTo outputFileURL: URL, from connections: [AVCaptureConnection], error: Error?) {
        recordingStartTime = nil
    }
}

private let recordingDelegate = MovieRecordingDelegate()

func camera_start_recording(path: RustString) -> CameraResultFFI {
    guard let output = movieOutput else {
        return .NotSupported
    }

    let url = URL(fileURLWithPath: path.toString())

    // Remove existing file if any
    try? FileManager.default.removeItem(at: url)

    recordingStartTime = Date()
    output.startRecording(to: url, recordingDelegate: recordingDelegate)
    return .Success
}

func camera_stop_recording() -> CameraResultFFI {
    guard let output = movieOutput else {
        return .NotSupported
    }

    if output.isRecording {
        output.stopRecording()
    }
    recordingStartTime = nil
    return .Success
}

func camera_get_recording_duration_ms() -> UInt64 {
    guard let startTime = recordingStartTime else { return 0 }
    return UInt64(Date().timeIntervalSince(startTime) * 1000)
}

func camera_start_raw_recording(path: RustString) -> CameraResultFFI {
    let outputPath = path.toString()
    if outputPath.isEmpty {
        return .OpenFailed
    }

    rawVideoLock.lock()
    defer { rawVideoLock.unlock() }

    if rawVideoFileHandle != nil {
        return .AlreadyInUse
    }

    let url = URL(fileURLWithPath: outputPath)
    try? FileManager.default.removeItem(at: url)
    guard FileManager.default.createFile(atPath: url.path, contents: nil) else {
        return .OpenFailed
    }
    guard let handle = FileHandle(forWritingAtPath: url.path) else {
        return .OpenFailed
    }

    writeRawVideoHeader(
        handle: handle,
        width: camera_get_resolution_width(),
        height: camera_get_resolution_height()
    )
    rawVideoFileHandle = handle
    rawVideoRecordingStartTime = Date()
    return .Success
}

func camera_stop_raw_recording() -> CameraResultFFI {
    rawVideoLock.lock()
    let handle = rawVideoFileHandle
    rawVideoFileHandle = nil
    rawVideoRecordingStartTime = nil
    rawVideoLock.unlock()

    handle?.closeFile()
    return .Success
}

func camera_get_raw_recording_duration_ms() -> UInt64 {
    rawVideoLock.lock()
    let startTime = rawVideoRecordingStartTime
    rawVideoLock.unlock()

    guard let startTime else { return 0 }
    return UInt64(Date().timeIntervalSince(startTime) * 1000)
}
