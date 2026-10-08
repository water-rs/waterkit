import Foundation
import AVFoundation
import CoreMedia
import CoreVideo
import Metal
#if os(iOS)
import UIKit
#endif

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
private var rawVideoInitialMatrix: UInt8?
private let rawVideoLock = NSLock()

// Frame callback - set from Rust: context, retained pixel buffer, timestamp
// (ns), clockwise rotation to upright (degrees), mirrored. The drop callback
// carries only the context: a dropped frame means every capture buffer is
// checked out, and Rust polls its GPU device so the buffers come back.
public typealias CameraFrameCallback = @convention(c) (
    UnsafeMutableRawPointer?,
    UInt64,
    UInt64,
    UInt32,
    Bool
) -> Void
public typealias CameraDropCallback = @convention(c) (UnsafeMutableRawPointer?) -> Void
private var frameCallback: CameraFrameCallback?
private var dropCallback: CameraDropCallback?
private var frameCallbackContext: UnsafeMutableRawPointer?
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
private var cachedSupportsDolbyVision: Bool = false
private var cachedSupportsStandardStabilization: Bool = false
private var cachedSupportsCinematicStabilization: Bool = false
private var cachedHasFlash: Bool = false
private var cachedHasTorch: Bool = false
private var cachedSupportsConcurrentMultiCam: Bool = false
private var cachedMaxConcurrentCameras: UInt8 = 1
private var cachedSupportsRawPhoto: Bool = false
private var cachedSupportsRawVideo: Bool = true
#if os(iOS)
private var cachedSdrFormat: AVCaptureDevice.Format?
private var cachedDolbyVisionFormat: AVCaptureDevice.Format?
#endif

// MARK: - Frame Orientation

// Clockwise angle, in degrees, that a capture connection would have to apply
// for its output to stand upright on the display. On iOS that follows the
// interface orientation of the app's foreground scene, so an app locked to
// portrait keeps portrait frames however the device is held. A Mac's display
// does not turn with gravity, so there it is the rotation coordinator's
// horizon-level angle (macOS 14), which turns only for a camera that can be
// rotated, such as an iPhone used as a webcam.
// `nil` until it is first known; frames are not delivered before then.
private var displayAngle: Int?
private let orientationLock = NSLock()
#if os(macOS)
// `AVCaptureDevice.RotationCoordinator` where available; stored untyped so the
// variable needs no availability annotation.
private var rotationCoordinator: AnyObject?
private var rotationObservation: NSKeyValueObservation?
#endif

private func normalizedDegrees(_ angle: Int) -> Int {
    return ((angle % 360) + 360) % 360
}

private func setDisplayAngle(_ angle: Int?) {
    orientationLock.lock()
    displayAngle = angle.map(normalizedDegrees)
    orientationLock.unlock()
}

// The rotation angle each legacy `AVCaptureVideoOrientation` stands for, as
// `videoRotationAngle` defines it.
private func rotationAngle(of orientation: AVCaptureVideoOrientation) -> Int {
    switch orientation {
    case .landscapeRight: return 0
    case .portrait: return 90
    case .landscapeLeft: return 180
    case .portraitUpsideDown: return 270
    @unknown default: return 0
    }
}

#if os(iOS)
// The capture rotation that matches the foreground scene's interface
// orientation, or `nil` while no scene is in the foreground. Main thread only.
private func interfaceRotationAngle() -> Int? {
    let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
    guard let scene = scenes.first(where: { $0.activationState == .foregroundActive }) else {
        return nil
    }
    switch scene.interfaceOrientation {
    case .landscapeRight: return 0
    case .portrait: return 90
    case .landscapeLeft: return 180
    case .portraitUpsideDown: return 270
    case .unknown: return nil
    @unknown default: return nil
    }
}

// Reads the interface orientation on the main thread, where UIKit allows it,
// keeping the last known angle while no scene is in the foreground.
private func refreshDisplayAngle() {
    DispatchQueue.main.async {
        if let angle = interfaceRotationAngle() {
            setDisplayAngle(angle)
        }
    }
}
#endif

private func startOrientationTracking(device: AVCaptureDevice) {
    setDisplayAngle(nil)
    #if os(iOS)
    refreshDisplayAngle()
    #else
    if #available(macOS 14.0, *) {
        let coordinator = AVCaptureDevice.RotationCoordinator(device: device, previewLayer: nil)
        rotationCoordinator = coordinator
        rotationObservation = coordinator.observe(
            \.videoRotationAngleForHorizonLevelCapture,
            options: [.initial, .new]
        ) { coordinator, _ in
            setDisplayAngle(Int(coordinator.videoRotationAngleForHorizonLevelCapture.rounded()))
        }
    } else {
        // A Mac's cameras do not turn with the machine.
        setDisplayAngle(0)
    }
    #endif
}

private func stopOrientationTracking() {
    #if os(macOS)
    rotationObservation?.invalidate()
    rotationObservation = nil
    rotationCoordinator = nil
    #endif
    setDisplayAngle(nil)
}

// The clockwise rotation, in degrees, that turns this connection's buffers
// upright on the display once any mirroring is undone, and whether the
// connection mirrors; `nil` while the display's orientation is unknown.
// Mirroring is about the output's vertical axis, after the rotation the
// connection applied.
private func frameOrientation(of connection: AVCaptureConnection) -> (UInt32, Bool)? {
    #if os(iOS)
    // The interface may turn at any frame; the next frame sees the change.
    refreshDisplayAngle()
    #endif
    let applied: Int
    if #available(iOS 17.0, macOS 14.0, *) {
        applied = Int(connection.videoRotationAngle.rounded())
    } else {
        applied = rotationAngle(of: connection.videoOrientation)
    }
    orientationLock.lock()
    let display = displayAngle
    orientationLock.unlock()
    guard let display else { return nil }
    return (UInt32(normalizedDegrees(display - applied)), connection.isVideoMirrored)
}

// MARK: - Frame Delegate

class CameraFrameDelegate: NSObject, AVCaptureVideoDataOutputSampleBufferDelegate {
    func captureOutput(_ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer, from connection: AVCaptureConnection) {
        guard let pixelBuffer = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }

        // The presentation time is the frame's capture-clock reading: the
        // capture session's synchronization clock, in host time.
        let pts = CMSampleBufferGetPresentationTimeStamp(sampleBuffer)
        precondition(pts.isNumeric, "a capture buffer's presentation time is numeric")
        let captureTimeNs = UInt64(CMTimeConvertScale(pts, timescale: 1_000_000_000, method: .roundHalfAwayFromZero).value)

        // A frame whose upright orientation is not known yet is not delivered:
        // it would reach the consumer turned the wrong way.
        if let (rotationDegrees, mirrored) = frameOrientation(of: connection) {
            deliver(pixelBuffer, timestampNs: captureTimeNs, rotationDegrees: rotationDegrees, mirrored: mirrored)
        }
        maybeWriteRawVideoFrame(pixelBuffer: pixelBuffer, timestampNs: captureTimeNs)
    }

    private func deliver(_ pixelBuffer: CVPixelBuffer, timestampNs: UInt64, rotationDegrees: UInt32, mirrored: Bool) {
        // Retain the CVPixelBuffer and hand that reference to Rust, which owns
        // it from here on and releases it when the frame built from it drops.
        let unmanaged = Unmanaged.passRetained(pixelBuffer)
        let handle = UInt64(UInt(bitPattern: unmanaged.toOpaque()))

        frameLock.lock()
        let callback = frameCallback
        let callbackContext = frameCallbackContext
        frameLock.unlock()

        if let callback {
            callback(callbackContext, handle, timestampNs, rotationDegrees, mirrored)
        } else {
            unmanaged.release()
        }
    }

    func captureOutput(_ output: AVCaptureOutput, didDrop sampleBuffer: CMSampleBuffer, from connection: AVCaptureConnection) {
        // Frame dropped - every capture buffer is checked out. Rust polls
        // its GPU device so the checked-out buffers come back.
        frameLock.lock()
        let callback = dropCallback
        let callbackContext = frameCallbackContext
        frameLock.unlock()
        callback?(callbackContext)
    }
}

private var frameDelegate = CameraFrameDelegate()

private func appendUInt32LE(_ value: UInt32, to data: inout Data) {
    var little = value.littleEndian
    withUnsafeBytes(of: &little) { data.append(contentsOf: $0) }
}

private func appendUInt64LE(_ value: UInt64, to data: inout Data) {
    var little = value.littleEndian
    withUnsafeBytes(of: &little) { data.append(contentsOf: $0) }
}

// The capture output's biplanar 4:2:0 pixel format, chosen when the camera
// opens: `420f` when the device offers it, otherwise `420v`.
private var capturePixelFormat: OSType = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange

private func rawVideoRangeCode() -> UInt8? {
    switch capturePixelFormat {
    case kCVPixelFormatType_420YpCbCr8BiPlanarFullRange:
        return 1
    case kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange:
        return 0
    default:
        return nil
    }
}

private func writeRawVideoHeader(
    handle: FileHandle,
    width: UInt32,
    height: UInt32,
    matrix: UInt8,
    range: UInt8
) {
    var header = Data()
    header.append(contentsOf: [UInt8(ascii: "W"), UInt8(ascii: "K"), UInt8(ascii: "R"), UInt8(ascii: "V")])
    header.append(contentsOf: [2, 3, matrix, range])
    appendUInt32LE(width, to: &header)
    appendUInt32LE(height, to: &header)
    appendUInt32LE(0, to: &header) // fps unknown from this layer
    handle.write(header)
}

// Appends one frame as its luma rows followed by its interleaved chroma rows,
// without row padding.
private func maybeWriteRawVideoFrame(pixelBuffer: CVPixelBuffer, timestampNs: UInt64) {
    rawVideoLock.lock()
    let isRecording = rawVideoFileHandle != nil
    rawVideoLock.unlock()

    guard isRecording else { return }

    let (matrixCode, matrixValue) = rawVideoMatrixCode(pixelBuffer)
    guard let matrixCode else {
        failRawVideoRecording(
            "missing or unsupported kCVImageBufferYCbCrMatrixKey value: \(matrixValue)"
        )
        return
    }
    guard let rangeCode = rawVideoRangeCode() else {
        failRawVideoRecording("unsupported capture pixel format for WKRV NV12 range")
        return
    }

    let lockResult = CVPixelBufferLockBaseAddress(pixelBuffer, .readOnly)
    if lockResult != kCVReturnSuccess {
        return
    }
    defer { CVPixelBufferUnlockBaseAddress(pixelBuffer, .readOnly) }

    // Luma holds one byte per pixel, chroma one Cb/Cr byte pair per 2x2 block.
    let planes = (0..<2).map { plane in
        (
            plane: plane,
            rowBytes: CVPixelBufferGetWidthOfPlane(pixelBuffer, plane) * (plane == 0 ? 1 : 2),
            rows: CVPixelBufferGetHeightOfPlane(pixelBuffer, plane)
        )
    }
    let payloadSize = planes.reduce(0) { $0 + $1.rowBytes * $1.rows }

    var payload = Data()
    payload.reserveCapacity(payloadSize)
    for plane in planes {
        guard let baseAddress = CVPixelBufferGetBaseAddressOfPlane(pixelBuffer, plane.plane) else {
            failRawVideoRecording("missing base address for raw video plane \(plane.plane)")
            return
        }
        let bytesPerRow = CVPixelBufferGetBytesPerRowOfPlane(pixelBuffer, plane.plane)
        for row in 0..<plane.rows {
            payload.append(
                Data(
                    bytes: baseAddress.advanced(by: row * bytesPerRow),
                    count: plane.rowBytes
                )
            )
        }
    }

    var frameHeader = Data()
    appendUInt64LE(timestampNs, to: &frameHeader)
    appendUInt32LE(UInt32(payloadSize), to: &frameHeader)

    var changedMatrix: UInt8?
    rawVideoLock.lock()
    if let handle = rawVideoFileHandle {
        if let firstMatrix = rawVideoInitialMatrix, firstMatrix != matrixCode {
            changedMatrix = firstMatrix
        } else {
            if rawVideoInitialMatrix == nil {
                writeRawVideoHeader(
                    handle: handle,
                    width: UInt32(CVPixelBufferGetWidth(pixelBuffer)),
                    height: UInt32(CVPixelBufferGetHeight(pixelBuffer)),
                    matrix: matrixCode,
                    range: rangeCode
                )
                rawVideoInitialMatrix = matrixCode
            }
            handle.write(frameHeader)
            handle.write(payload)
        }
    }
    rawVideoLock.unlock()

    if let changedMatrix {
        failRawVideoRecording(
            "raw video YCbCr matrix changed from H.273 code \(changedMatrix) to \(matrixCode)"
        )
    }
}

private func rawVideoMatrixCode(_ pixelBuffer: CVPixelBuffer) -> (UInt8?, String) {
    var attachmentMode = CVAttachmentMode.shouldPropagate
    guard let attachment = CVBufferGetAttachment(
        pixelBuffer,
        kCVImageBufferYCbCrMatrixKey,
        &attachmentMode
    ) else {
        return (nil, "<missing>")
    }
    let attachmentValue = attachment.takeUnretainedValue()
    guard let value = attachmentValue as? String else {
        return (nil, String(describing: attachmentValue))
    }

    if value == (kCVImageBufferYCbCrMatrix_ITU_R_601_4 as String) {
        return (6, value)
    }
    if value == (kCVImageBufferYCbCrMatrix_ITU_R_709_2 as String) {
        return (1, value)
    }
    if value == (kCVImageBufferYCbCrMatrix_ITU_R_2020 as String) {
        return (9, value)
    }
    return (nil, value)
}

private func detachRawVideoFileHandle() -> FileHandle? {
    rawVideoLock.lock()
    let handle = rawVideoFileHandle
    rawVideoFileHandle = nil
    rawVideoInitialMatrix = nil
    rawVideoRecordingStartTime = nil
    rawVideoLock.unlock()
    return handle
}

private func failRawVideoRecording(_ message: String) {
    NSLog("WaterkitCamera RAW video: %@", message)
    detachRawVideoFileHandle()?.closeFile()
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

// Why the last `camera_open` returned `.OpenFailed`, for the Rust error.
private var openFailure = ""

private func openFailed(_ reason: String) -> CameraResultFFI {
    openFailure = reason
    return .OpenFailed
}

func camera_open_failure() -> RustString {
    return openFailure.intoRustString()
}

// A pixel format's four-character code, such as `420f`.
private func fourCharCode(_ format: OSType) -> String {
    let bytes = [24, 16, 8, 0].map { UInt8((format >> $0) & 0xff) }
    return String(decoding: bytes, as: UTF8.self)
}

func camera_open(device_id: RustString) -> CameraResultFFI {
    guard captureSession == nil else {
        return .AlreadyInUse
    }
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
    #if os(iOS)
    session.automaticallyConfiguresCaptureDeviceForWideColor = false
    #endif

    do {
        let input = try AVCaptureDeviceInput(device: device)
        if session.canAddInput(input) {
            session.addInput(input)
        } else {
            return openFailed("the capture session cannot take \(device.localizedName) as its input")
        }
    } catch {
        return openFailed("\(device.localizedName) cannot be opened: \(error.localizedDescription)")
    }

    let output = AVCaptureVideoDataOutput()
    // Keep the camera's native biplanar 4:2:0 layout so frames reach the GPU
    // as their IOSurface planes, without a conversion: full range when the
    // device offers it, video range otherwise.
    let offered = output.availableVideoPixelFormatTypes
    if offered.contains(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange) {
        capturePixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
    } else if offered.contains(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange) {
        capturePixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
    } else {
        let formats = offered.map(fourCharCode).joined(separator: ", ")
        return openFailed(
            "\(device.localizedName) offers neither 420f nor 420v frames, which the camera needs to "
                + "hand them to the GPU as they are; it offers [\(formats)]"
        )
    }
    output.videoSettings = [
        kCVPixelBufferPixelFormatTypeKey as String: capturePixelFormat
    ]
    output.setSampleBufferDelegate(frameDelegate, queue: frameQueue)
    output.alwaysDiscardsLateVideoFrames = true

    if session.canAddOutput(output) {
        session.addOutput(output)
    } else {
        return openFailed("the capture session cannot add a video data output for \(device.localizedName)")
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
    startOrientationTracking(device: device)

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

func camera_close() -> CameraResultFFI {
    _ = camera_stop()
    stopOrientationTracking()
    captureSession = nil
    videoOutput = nil
    photoOutput = nil
    movieOutput = nil
    currentDevice = nil
    return .Success
}

func camera_is_streaming() -> Bool {
    return captureSession?.isRunning ?? false
}

// MARK: - Frame Callback

@_cdecl("camera_set_frame_callback")
public func camera_set_frame_callback(
    context: UnsafeMutableRawPointer?,
    callback: @escaping CameraFrameCallback,
    onDrop: @escaping CameraDropCallback
) {
    frameLock.lock()
    frameCallbackContext = context
    frameCallback = callback
    dropCallback = onDrop
    frameLock.unlock()
}

@_cdecl("camera_clear_frame_callback")
public func camera_clear_frame_callback() {
    frameLock.lock()
    frameCallback = nil
    dropCallback = nil
    frameCallbackContext = nil
    frameLock.unlock()
    frameQueue.sync {}
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

    // Apple records 10-bit HLG with Dolby Vision metadata. HDR10/PQ is not an
    // AVCaptureMovieFileOutput profile, so expose only the profile we can
    // actually configure and record.
    let activeDimensions = CMVideoFormatDescriptionGetDimensions(format.formatDescription)
    let closestFormat = { (colorSpace: AVCaptureColorSpace) -> AVCaptureDevice.Format? in
        device.formats
            .filter { candidate in candidate.supportedColorSpaces.contains(colorSpace) }
            .min { lhs, rhs in
                let lhsDimensions = CMVideoFormatDescriptionGetDimensions(lhs.formatDescription)
                let rhsDimensions = CMVideoFormatDescriptionGetDimensions(rhs.formatDescription)
                let lhsDistance = abs(lhsDimensions.width - activeDimensions.width) +
                    abs(lhsDimensions.height - activeDimensions.height)
                let rhsDistance = abs(rhsDimensions.width - activeDimensions.width) +
                    abs(rhsDimensions.height - activeDimensions.height)
                return lhsDistance < rhsDistance
            }
    }
    cachedSdrFormat = closestFormat(.sRGB)
    // HLG_BT2020 arrived in iOS 14.1, one point release after this module's
    // deployment target. Below it there is simply no Dolby Vision format, which
    // the capability flag below already reports as unsupported.
    if #available(iOS 14.1, macCatalyst 14.1, macOS 11.0, *) {
        cachedDolbyVisionFormat = closestFormat(.HLG_BT2020)
    } else {
        cachedDolbyVisionFormat = nil
    }
    cachedSupportsDolbyVision =
        cachedDolbyVisionFormat != nil &&
        (movieOutput?.availableVideoCodecTypes.contains(.hevc) ?? false)

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
    default: return .Unsupported
    }

    guard device.isExposureModeSupported(avMode) else { return .Unsupported }

    do {
        try device.lockForConfiguration()
        device.exposureMode = avMode
        device.unlockForConfiguration()
        return .Success
    } catch {
        return .OpenFailed
    }
    #else
    return .Unsupported
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
    return .Unsupported
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
    return .Unsupported
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
    return .Unsupported
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
    default: return .Unsupported
    }

    guard device.isFocusModeSupported(avMode) else { return .Unsupported }

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
    guard device.isLockingFocusWithCustomLensPositionSupported else { return .Unsupported }

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
    return .Unsupported
    #endif
}

func camera_set_focus_point(x: Float, y: Float) -> CameraResultFFI {
    guard let device = currentDevice else { return .OpenFailed }
    guard device.isFocusPointOfInterestSupported else { return .Unsupported }

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

    guard device.isWhiteBalanceModeSupported(avMode) else { return .Unsupported }

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
    return .Unsupported
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
    return .Unsupported
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
    guard device.hasTorch else { return .Unsupported }

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

func camera_set_dynamic_range(profile: UInt8) -> CameraResultFFI {
    #if os(iOS)
    guard let device = currentDevice else {
        return .OpenFailed
    }
    guard movieOutput?.isRecording != true else {
        return .AlreadyInUse
    }
    guard let session = captureSession else {
        return .OpenFailed
    }

    let format: AVCaptureDevice.Format
    let colorSpace: AVCaptureColorSpace
    let codec: AVVideoCodecType
    switch profile {
    case 0:
        guard let sdrFormat = cachedSdrFormat else { return .Unsupported }
        format = sdrFormat
        colorSpace = .sRGB
        codec = .h264
    case 3:
        guard cachedSupportsDolbyVision, let dolbyFormat = cachedDolbyVisionFormat else {
            return .Unsupported
        }
        guard #available(iOS 14.1, macCatalyst 14.1, macOS 11.0, *) else {
            return .Unsupported
        }
        format = dolbyFormat
        colorSpace = .HLG_BT2020
        codec = .hevc
    case 1, 2:
        return .Unsupported
    default:
        return .Unsupported
    }

    do {
        session.beginConfiguration()
        defer { session.commitConfiguration() }
        session.sessionPreset = .inputPriority
        try device.lockForConfiguration()
        device.activeFormat = format
        device.activeColorSpace = colorSpace
        device.automaticallyAdjustsVideoHDREnabled = false
        if format.isVideoHDRSupported {
            device.isVideoHDREnabled = profile != 0
        }
        device.unlockForConfiguration()

        guard let output = movieOutput, let connection = output.connection(with: .video) else {
            return .OpenFailed
        }
        guard output.availableVideoCodecTypes.contains(codec) else {
            return .Unsupported
        }
        output.setOutputSettings([AVVideoCodecKey: codec], for: connection)
        return .Success
    } catch {
        return .OpenFailed
    }
    #else
    return profile == 0 ? .Success : .Unsupported
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
    default: return .Unsupported
    }

    guard connection.isVideoStabilizationSupported else { return .Unsupported }
    connection.preferredVideoStabilizationMode = avMode

    return .Success
    #else
    return .Unsupported
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
        return .Unsupported
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
        return .Unsupported
    }
    guard let rawFormat = output.availableRawPhotoPixelFormatTypes.first else {
        return .Unsupported
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
    return .Unsupported
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
        return .Unsupported
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
        return .Unsupported
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

    rawVideoFileHandle = handle
    rawVideoInitialMatrix = nil
    rawVideoRecordingStartTime = Date()
    return .Success
}

func camera_stop_raw_recording() -> CameraResultFFI {
    detachRawVideoFileHandle()?.closeFile()
    return .Success
}

func camera_get_raw_recording_duration_ms() -> UInt64 {
    rawVideoLock.lock()
    let startTime = rawVideoRecordingStartTime
    rawVideoLock.unlock()

    guard let startTime else { return 0 }
    return UInt64(Date().timeIntervalSince(startTime) * 1000)
}