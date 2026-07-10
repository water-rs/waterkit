import AVFoundation
import AVKit
import CoreMedia
import CoreVideo
import Foundation
import Metal
import OSLog

private let logger = Logger(subsystem: "dev.waterui", category: "WaterKitVideoPiP")
private let pictureInPictureFrameInterval: TimeInterval = 1.0 / 30.0

func waterkit_video_swift_bridge_marker() -> Bool {
    true
}

typealias RenderFrameFn = @convention(c) (
    UnsafeMutableRawPointer?,
    UnsafeMutableRawPointer?,
    UInt32,
    UInt32
) -> Bool

typealias SetExternalRenderingFn = @convention(c) (
    UnsafeMutableRawPointer?,
    Bool
) -> Void

private enum WaterKitPictureInPictureEnterResult: Int32 {
    case success = 0
    case unsupported = 1
    case hostNotRegistered = 2
    case notPossible = 3
    case startFailed = 4
}

private enum WaterKitPictureInPictureCommandKind: Int32 {
    case none = 0
    case play = 1
    case pause = 2
    case seekForward = 3
    case seekBackward = 4
}

private struct WaterKitPictureInPictureCommandRecord {
    let kind: WaterKitPictureInPictureCommandKind
    let valueSeconds: Double
}

private func runOnMain<T>(_ body: () -> T) -> T {
    if Thread.isMainThread {
        return body()
    }

    return DispatchQueue.main.sync {
        body()
    }
}

/// Runs a result-less bridge operation on the main thread without blocking the
/// caller. State pushes (host registration, controller sync, teardown) need
/// main-thread execution and in-order delivery — the serial main queue gives
/// both — but nothing waits on them, so a synchronous hop is a deadlock hazard:
/// a caller off the main thread blocks forever whenever the main thread is not
/// draining its queue (Rust's libtest parks it while a test drops a
/// `VideoRenderer`, which pushes the PiP clear from the test thread).
private func runOnMainAsync(_ body: @escaping () -> Void) {
    if Thread.isMainThread {
        body()
        return
    }

    DispatchQueue.main.async {
        body()
    }
}

private final class PictureInPictureHostRegistration {
    let hostId: UInt64
    let userData: UnsafeMutableRawPointer?
    var renderFrame: RenderFrameFn
    var setExternalRendering: SetExternalRenderingFn
    var active = false
    var playing = false
    var aspectRatio: (UInt32, UInt32)?
    var commandQueue: [WaterKitPictureInPictureCommandRecord] = []

    init(
        hostId: UInt64,
        userData: UnsafeMutableRawPointer?,
        renderFrame: @escaping RenderFrameFn,
        setExternalRendering: @escaping SetExternalRenderingFn
    ) {
        self.hostId = hostId
        self.userData = userData
        self.renderFrame = renderFrame
        self.setExternalRendering = setExternalRendering
    }
}

@available(iOS 15.0, macOS 12.0, *)
private final class PictureInPictureSession: NSObject,
    AVPictureInPictureControllerDelegate,
    AVPictureInPictureSampleBufferPlaybackDelegate
{
    let host: PictureInPictureHostRegistration
    let manager: PictureInPictureManager
    private let device: MTLDevice
    private let textureCache: CVMetalTextureCache
    private let displayLayer: AVSampleBufferDisplayLayer
    private var controller: AVPictureInPictureController!
    private var timer: Timer?
    private var renderSize: CMVideoDimensions
    private var pixelBuffer: CVPixelBuffer?
    private var metalTexture: MTLTexture?
    private var formatDescription: CMVideoFormatDescription?
    private var frameIndex: Int64 = 0

    init?(
        host: PictureInPictureHostRegistration,
        manager: PictureInPictureManager
    ) {
        guard let device = MTLCreateSystemDefaultDevice() else {
            logger.error("Failed to create Metal device for picture in picture")
            return nil
        }

        var textureCache: CVMetalTextureCache?
        let cacheStatus = CVMetalTextureCacheCreate(
            kCFAllocatorDefault,
            nil,
            device,
            nil,
            &textureCache
        )
        guard cacheStatus == kCVReturnSuccess, let textureCache else {
            logger.error("CVMetalTextureCacheCreate failed: \(cacheStatus)")
            return nil
        }

        self.host = host
        self.manager = manager
        self.device = device
        self.textureCache = textureCache
        self.displayLayer = AVSampleBufferDisplayLayer()
        self.renderSize = Self.initialRenderSize(aspectRatio: host.aspectRatio)
        self.displayLayer.videoGravity = .resizeAspect
        self.displayLayer.isOpaque = true

        super.init()

        let contentSource = AVPictureInPictureController.ContentSource(
            sampleBufferDisplayLayer: displayLayer,
            playbackDelegate: self
        )
        let controller = AVPictureInPictureController(contentSource: contentSource)
        self.controller = controller
        self.controller.delegate = self
        self.host.setExternalRendering(self.host.userData, true)
        self.controller.requiresLinearPlayback = false
        _ = ensureRenderTarget(size: self.renderSize)
        enqueueFrame()
        self.controller.invalidatePlaybackState()
    }

    deinit {
        timer?.invalidate()
        host.setExternalRendering(host.userData, false)
    }

    func start() -> WaterKitPictureInPictureEnterResult {
        guard AVPictureInPictureController.isPictureInPictureSupported() else {
            return .unsupported
        }
        guard host.active else {
            return .notPossible
        }
        guard ensureRenderTarget(size: renderSize) else {
            return .startFailed
        }

        enqueueFrame()
        startFramePump()
        controller.invalidatePlaybackState()

        guard controller.isPictureInPicturePossible else {
            logger.error("Picture in picture is not possible for host \(self.host.hostId)")
            stopFramePump()
            host.setExternalRendering(host.userData, false)
            return .notPossible
        }

        controller.startPictureInPicture()
        return .success
    }

    func updateFromHostState() {
        if let aspectRatio = host.aspectRatio {
            let nextRenderSize = Self.initialRenderSize(aspectRatio: aspectRatio)
            if nextRenderSize.width != renderSize.width || nextRenderSize.height != renderSize.height {
                renderSize = nextRenderSize
                _ = ensureRenderTarget(size: renderSize)
            }
        }

        if !host.active && controller.isPictureInPictureActive {
            controller.stopPictureInPicture()
        }

        controller.invalidatePlaybackState()
    }

    var isActive: Bool {
        controller.isPictureInPictureActive
    }

    private func startFramePump() {
        guard timer == nil else { return }

        let timer = Timer(
            timeInterval: pictureInPictureFrameInterval,
            repeats: true
        ) { [weak self] _ in
            guard let self else { return }
            self.enqueueFrame()
        }
        RunLoop.main.add(timer, forMode: .common)
        self.timer = timer
    }

    private func stopFramePump() {
        timer?.invalidate()
        timer = nil
    }

    private func enqueueFrame() {
        guard host.active else { return }
        guard ensureRenderTarget(size: renderSize) else { return }
        guard let pixelBuffer, let metalTexture else { return }

        if !host.renderFrame(
            host.userData,
            Unmanaged.passUnretained(metalTexture).toOpaque(),
            UInt32(renderSize.width),
            UInt32(renderSize.height)
        ) {
            logger.debug("Render callback returned false for PiP host \(self.host.hostId)")
            return
        }

        if displayLayer.status == .failed {
            logger.warning("Sample buffer display layer failed, flushing for PiP host \(self.host.hostId)")
            displayLayer.flushAndRemoveImage()
        }

        guard let sampleBuffer = makeSampleBuffer(pixelBuffer: pixelBuffer) else {
            logger.error("Failed to create sample buffer for PiP host \(self.host.hostId)")
            return
        }

        displayLayer.enqueue(sampleBuffer)
        frameIndex &+= 1
    }

    private func ensureRenderTarget(size: CMVideoDimensions) -> Bool {
        if let pixelBuffer,
           CVPixelBufferGetWidth(pixelBuffer) == Int(size.width),
           CVPixelBufferGetHeight(pixelBuffer) == Int(size.height),
           metalTexture != nil,
           formatDescription != nil
        {
            return true
        }

        var pixelBuffer: CVPixelBuffer?
        let attributes: [CFString: Any] = [
            kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_32BGRA,
            kCVPixelBufferWidthKey: Int(size.width),
            kCVPixelBufferHeightKey: Int(size.height),
            kCVPixelBufferMetalCompatibilityKey: true,
            kCVPixelBufferIOSurfacePropertiesKey: [:] as CFDictionary,
        ]
        let status = CVPixelBufferCreate(
            kCFAllocatorDefault,
            Int(size.width),
            Int(size.height),
            kCVPixelFormatType_32BGRA,
            attributes as CFDictionary,
            &pixelBuffer
        )
        guard status == kCVReturnSuccess, let pixelBuffer else {
            logger.error("CVPixelBufferCreate failed: \(status)")
            return false
        }

        var cvMetalTexture: CVMetalTexture?
        let textureStatus = CVMetalTextureCacheCreateTextureFromImage(
            kCFAllocatorDefault,
            textureCache,
            pixelBuffer,
            nil,
            .bgra8Unorm,
            Int(size.width),
            Int(size.height),
            0,
            &cvMetalTexture
        )
        guard
            textureStatus == kCVReturnSuccess,
            let cvMetalTexture,
            let metalTexture = CVMetalTextureGetTexture(cvMetalTexture)
        else {
            logger.error("CVMetalTextureCacheCreateTextureFromImage failed: \(textureStatus)")
            return false
        }

        var formatDescription: CMVideoFormatDescription?
        let formatStatus = CMVideoFormatDescriptionCreateForImageBuffer(
            allocator: kCFAllocatorDefault,
            imageBuffer: pixelBuffer,
            formatDescriptionOut: &formatDescription
        )
        guard formatStatus == noErr, let formatDescription else {
            logger.error("CMVideoFormatDescriptionCreateForImageBuffer failed: \(formatStatus)")
            return false
        }

        self.pixelBuffer = pixelBuffer
        self.metalTexture = metalTexture
        self.formatDescription = formatDescription
        return true
    }

    private func makeSampleBuffer(pixelBuffer: CVPixelBuffer) -> CMSampleBuffer? {
        guard let formatDescription else { return nil }

        let timeScale: Int32 = 600
        let presentationTime = CMTime(value: frameIndex, timescale: timeScale)
        var timing = CMSampleTimingInfo(
            duration: CMTime(value: 1, timescale: timeScale),
            presentationTimeStamp: presentationTime,
            decodeTimeStamp: .invalid
        )

        var sampleBuffer: CMSampleBuffer?
        let status = CMSampleBufferCreateReadyWithImageBuffer(
            allocator: kCFAllocatorDefault,
            imageBuffer: pixelBuffer,
            formatDescription: formatDescription,
            sampleTiming: &timing,
            sampleBufferOut: &sampleBuffer
        )
        guard status == noErr, let sampleBuffer else {
            logger.error("CMSampleBufferCreateReadyWithImageBuffer failed: \(status)")
            return nil
        }

        if let attachments = CMSampleBufferGetSampleAttachmentsArray(
            sampleBuffer,
            createIfNecessary: true
        ) {
            let attachment = unsafeBitCast(
                CFArrayGetValueAtIndex(attachments, 0),
                to: CFMutableDictionary.self
            )
            CFDictionarySetValue(
                attachment,
                Unmanaged.passUnretained(kCMSampleAttachmentKey_DisplayImmediately).toOpaque(),
                Unmanaged.passUnretained(kCFBooleanTrue).toOpaque()
            )
        }

        return sampleBuffer
    }

    private static func initialRenderSize(
        aspectRatio: (UInt32, UInt32)?
    ) -> CMVideoDimensions {
        let defaultWidth: Int32 = 960
        let defaultHeight: Int32 = 540
        guard let aspectRatio, aspectRatio.0 > 0, aspectRatio.1 > 0 else {
            return CMVideoDimensions(width: defaultWidth, height: defaultHeight)
        }

        let width = Double(defaultWidth)
        let ratio = Double(aspectRatio.1) / Double(aspectRatio.0)
        let height = max(1, Int32((width * ratio).rounded()))
        return CMVideoDimensions(width: defaultWidth, height: height)
    }

    func pictureInPictureController(
        _ pictureInPictureController: AVPictureInPictureController,
        setPlaying playing: Bool
    ) {
        host.commandQueue.append(
            WaterKitPictureInPictureCommandRecord(
                kind: playing ? .play : .pause,
                valueSeconds: 0
            )
        )
    }

    func pictureInPictureControllerTimeRangeForPlayback(
        _ pictureInPictureController: AVPictureInPictureController
    ) -> CMTimeRange {
        guard host.active else {
            return .invalid
        }

        return CMTimeRange(start: .zero, duration: .positiveInfinity)
    }

    func pictureInPictureControllerIsPlaybackPaused(
        _ pictureInPictureController: AVPictureInPictureController
    ) -> Bool {
        !host.playing
    }

    func pictureInPictureController(
        _ pictureInPictureController: AVPictureInPictureController,
        didTransitionToRenderSize newRenderSize: CMVideoDimensions
    ) {
        guard newRenderSize.width > 0, newRenderSize.height > 0 else { return }
        renderSize = newRenderSize
        _ = ensureRenderTarget(size: newRenderSize)
    }

    func pictureInPictureController(
        _ pictureInPictureController: AVPictureInPictureController,
        skipByInterval skipInterval: CMTime,
        completion completionHandler: @escaping () -> Void
    ) {
        let seconds = skipInterval.seconds
        if seconds > 0 {
            host.commandQueue.append(
                WaterKitPictureInPictureCommandRecord(
                    kind: .seekForward,
                    valueSeconds: seconds
                )
            )
        } else if seconds < 0 {
            host.commandQueue.append(
                WaterKitPictureInPictureCommandRecord(
                    kind: .seekBackward,
                    valueSeconds: -seconds
                )
            )
        }
        completionHandler()
    }

    func pictureInPictureControllerShouldProhibitBackgroundAudioPlayback(
        _ pictureInPictureController: AVPictureInPictureController
    ) -> Bool {
        false
    }

    func pictureInPictureControllerWillStartPictureInPicture(
        _ pictureInPictureController: AVPictureInPictureController
    ) {
        startFramePump()
    }

    func pictureInPictureControllerWillStopPictureInPicture(
        _ pictureInPictureController: AVPictureInPictureController
    ) {
        stopFramePump()
    }

    func pictureInPictureControllerDidStopPictureInPicture(
        _ pictureInPictureController: AVPictureInPictureController
    ) {
        manager.sessionDidStop(hostId: host.hostId)
    }

    func pictureInPictureController(
        _ pictureInPictureController: AVPictureInPictureController,
        failedToStartPictureInPictureWithError error: Error
    ) {
        logger.error("PiP failed to start for host \(self.host.hostId): \(error.localizedDescription)")
        manager.sessionDidStop(hostId: host.hostId)
    }
}

@available(iOS 15.0, macOS 12.0, *)
private final class PictureInPictureManager {
    static let shared = PictureInPictureManager()

    private var hosts: [UInt64: PictureInPictureHostRegistration] = [:]
    private var activeSession: PictureInPictureSession?

    func registerHost(
        hostId: UInt64,
        userData: UnsafeMutableRawPointer?,
        renderFrame: @escaping RenderFrameFn,
        setExternalRendering: @escaping SetExternalRenderingFn
    ) {
        if let host = hosts[hostId] {
            host.renderFrame = renderFrame
            host.setExternalRendering = setExternalRendering
            return
        }

        hosts[hostId] = PictureInPictureHostRegistration(
            hostId: hostId,
            userData: userData,
            renderFrame: renderFrame,
            setExternalRendering: setExternalRendering
        )
    }

    func unregisterHost(hostId: UInt64) {
        if activeSession?.host.hostId == hostId {
            activeSession = nil
        }
        hosts.removeValue(forKey: hostId)
    }

    func syncHostState(
        hostId: UInt64,
        active: Bool,
        playing: Bool,
        aspectWidth: UInt32,
        aspectHeight: UInt32
    ) {
        let host = hosts[hostId] ?? PictureInPictureHostRegistration(
            hostId: hostId,
            userData: nil,
            renderFrame: { _, _, _, _ in false },
            setExternalRendering: { _, _ in }
        )
        host.active = active
        host.playing = playing
        host.aspectRatio = aspectWidth > 0 && aspectHeight > 0
            ? (aspectWidth, aspectHeight)
            : nil
        hosts[hostId] = host

        if activeSession?.host.hostId == hostId {
            activeSession?.updateFromHostState()
        }
    }

    func enter(hostId: UInt64) -> WaterKitPictureInPictureEnterResult {
        guard AVPictureInPictureController.isPictureInPictureSupported() else {
            return .unsupported
        }
        guard let host = hosts[hostId], host.userData != nil else {
            return .hostNotRegistered
        }

        if let activeSession, activeSession.host.hostId == hostId, activeSession.isActive {
            return .success
        }

        activeSession = nil
        guard let session = PictureInPictureSession(host: host, manager: self) else {
            return .startFailed
        }
        activeSession = session
        return session.start()
    }

    func isActive(hostId: UInt64) -> Bool {
        activeSession?.host.hostId == hostId && activeSession?.isActive == true
    }

    func pollCommand(hostId: UInt64) -> WaterKitPictureInPictureCommandRecord? {
        guard let host = hosts[hostId], !host.commandQueue.isEmpty else {
            return nil
        }

        return host.commandQueue.removeFirst()
    }

    func sessionDidStop(hostId: UInt64) {
        if activeSession?.host.hostId == hostId {
            activeSession = nil
        }
    }
}

@_cdecl("waterkit_video_apple_pip_bridge_register_host")
func waterkit_video_apple_pip_bridge_register_host(
    _ hostId: UInt64,
    _ userData: UnsafeMutableRawPointer?,
    _ renderFrame: @escaping RenderFrameFn,
    _ setExternalRendering: @escaping SetExternalRenderingFn
) {
    runOnMainAsync {
        if #available(iOS 15.0, macOS 12.0, *) {
            PictureInPictureManager.shared.registerHost(
                hostId: hostId,
                userData: userData,
                renderFrame: renderFrame,
                setExternalRendering: setExternalRendering
            )
        }
    }
}

@_cdecl("waterkit_video_apple_pip_bridge_unregister_host")
func waterkit_video_apple_pip_bridge_unregister_host(_ hostId: UInt64) {
    runOnMainAsync {
        if #available(iOS 15.0, macOS 12.0, *) {
            PictureInPictureManager.shared.unregisterHost(hostId: hostId)
        }
    }
}

@_cdecl("waterkit_video_apple_pip_bridge_sync_host_state")
func waterkit_video_apple_pip_bridge_sync_host_state(
    _ hostId: UInt64,
    _ active: Bool,
    _ playing: Bool,
    _ aspectWidth: UInt32,
    _ aspectHeight: UInt32
) {
    runOnMainAsync {
        if #available(iOS 15.0, macOS 12.0, *) {
            PictureInPictureManager.shared.syncHostState(
                hostId: hostId,
                active: active,
                playing: playing,
                aspectWidth: aspectWidth,
                aspectHeight: aspectHeight
            )
        }
    }
}

@_cdecl("waterkit_video_apple_pip_bridge_enter")
func waterkit_video_apple_pip_bridge_enter(_ hostId: UInt64) -> Int32 {
    runOnMain {
        if #available(iOS 15.0, macOS 12.0, *) {
            return PictureInPictureManager.shared.enter(hostId: hostId).rawValue
        }
        return WaterKitPictureInPictureEnterResult.unsupported.rawValue
    }
}

@_cdecl("waterkit_video_apple_pip_bridge_is_active")
func waterkit_video_apple_pip_bridge_is_active(_ hostId: UInt64) -> Bool {
    runOnMain {
        if #available(iOS 15.0, macOS 12.0, *) {
            return PictureInPictureManager.shared.isActive(hostId: hostId)
        }
        return false
    }
}

@_cdecl("waterkit_video_apple_pip_bridge_poll_command_kind")
func waterkit_video_apple_pip_bridge_poll_command_kind(
    _ hostId: UInt64,
    _ kindOut: UnsafeMutablePointer<Int32>?,
    _ valueSecsOut: UnsafeMutablePointer<Double>?
) {
    let command = runOnMain {
        if #available(iOS 15.0, macOS 12.0, *) {
            return PictureInPictureManager.shared.pollCommand(hostId: hostId)
        }
        return nil
    }
    kindOut?.pointee = command?.kind.rawValue ?? WaterKitPictureInPictureCommandKind.none.rawValue
    valueSecsOut?.pointee = command?.valueSeconds ?? 0
}
