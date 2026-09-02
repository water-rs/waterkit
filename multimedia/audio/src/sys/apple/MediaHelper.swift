import Foundation
import MediaPlayer
import AVFoundation
import OSLog

private let logger = Logger(subsystem: "dev.waterui", category: "WaterKitMedia")

private enum MediaCommandKind: UInt8 {
    case none = 0
    case play = 1
    case pause = 2
    case playPause = 3
    case stop = 4
    case next = 5
    case previous = 6
    case seek = 7
    case seekForward = 8
    case seekBackward = 9
    case audioFocusGained = 10
    case audioFocusLost = 11
    case audioFocusLostTransient = 12
    case audioFocusLostDuck = 13
    case audioBecomingNoisy = 14
}

private struct MediaCommandRecord {
    let kind: MediaCommandKind
    let valueSeconds: Double
}

private final class MediaCommandChannel {
    private let condition = NSCondition()
    private var commands: [MediaCommandRecord] = []
    private var closed = false

    func send(_ command: MediaCommandRecord) {
        condition.lock()
        guard !closed else {
            condition.unlock()
            return
        }
        commands.append(command)
        condition.signal()
        condition.unlock()
    }

    func receive() -> MediaCommandRecord? {
        condition.lock()
        while commands.isEmpty && !closed {
            condition.wait()
        }
        let command = commands.isEmpty ? nil : commands.removeFirst()
        condition.unlock()
        return command
    }

    func close() {
        condition.lock()
        closed = true
        condition.broadcast()
        condition.unlock()
    }
}

private final class MediaSessionRegistry {
    static let shared = MediaSessionRegistry()

    private let lock = NSLock()
    private var nextSessionId: UInt64 = 1
    private var channels: [UInt64: MediaCommandChannel] = [:]
    private var sessionOrder: [UInt64] = []
    private var activeSessionId: UInt64?
    private var commandHandlerRegistered = false

    func createSession() -> UInt64 {
        lock.lock()
        let sessionId = nextSessionId
        nextSessionId = nextSessionId.addingReportingOverflow(1).partialValue
        precondition(nextSessionId != 0, "Apple media session identifier space exhausted")
        channels[sessionId] = MediaCommandChannel()
        sessionOrder.append(sessionId)
        activeSessionId = sessionId
        lock.unlock()
        return sessionId
    }

    func activate(_ sessionId: UInt64) -> Bool {
        lock.lock()
        let exists = channels[sessionId] != nil
        if exists && activeSessionId != sessionId {
            sessionOrder.removeAll { $0 == sessionId }
            sessionOrder.append(sessionId)
            activeSessionId = sessionId
        }
        lock.unlock()
        return exists
    }

    func isActive(_ sessionId: UInt64) -> Bool {
        lock.lock()
        let active = activeSessionId == sessionId
        lock.unlock()
        return active
    }

    func waitForCommand(sessionId: UInt64) -> MediaCommandRecord? {
        lock.lock()
        let channel = channels[sessionId]
        lock.unlock()
        return channel?.receive()
    }

    func sendToActive(_ kind: MediaCommandKind, valueSeconds: Double = 0) {
        lock.lock()
        let channel = activeSessionId.flatMap { channels[$0] }
        lock.unlock()
        channel?.send(MediaCommandRecord(kind: kind, valueSeconds: valueSeconds))
    }

    func closeSession(_ sessionId: UInt64) -> Bool {
        lock.lock()
        let channel = channels.removeValue(forKey: sessionId)
        sessionOrder.removeAll { $0 == sessionId }
        if activeSessionId == sessionId {
            activeSessionId = sessionOrder.last
        }
        lock.unlock()
        channel?.close()
        return channel != nil
    }

    func hasSessions() -> Bool {
        lock.lock()
        let result = !channels.isEmpty
        lock.unlock()
        return result
    }

    func registerCommandHandlerIfNeeded() {
        lock.lock()
        let shouldRegister = !commandHandlerRegistered
        commandHandlerRegistered = true
        lock.unlock()
        if shouldRegister {
            registerMediaCommandHandlers()
        }
    }
}

#if os(iOS)
private var systemEventObserverTokens: [NSObjectProtocol] = []
private var systemEventObserversRegistered = false
#endif

#if os(macOS)
private var silentPlayer: AVAudioPlayer?
private let silentPlayerDelegate = SilentPlayerDelegate()
#endif

// MARK: - FFI Functions

func media_session_init() -> MediaSessionHandleFFI {
    // On macOS, we need to briefly "play" something to activate the audio session
    // so that MPNowPlayingInfoCenter shows up in Control Center
    #if os(macOS)
    activateAudioSessionWithSilence()
    #endif
    let sessionId = MediaSessionRegistry.shared.createSession()
    MediaSessionRegistry.shared.registerCommandHandlerIfNeeded()
    #if os(iOS)
    registerSystemEventObservers()
    #endif
    return MediaSessionHandleFFI(result: .Success, session_id: sessionId)
}

#if os(macOS)
private final class SilentPlayerDelegate: NSObject, AVAudioPlayerDelegate {
    func audioPlayerDidFinishPlaying(_ player: AVAudioPlayer, successfully flag: Bool) {
        if silentPlayer === player {
            silentPlayer = nil
        }
    }

    func audioPlayerDecodeErrorDidOccur(_ player: AVAudioPlayer, error: Error?) {
        if silentPlayer === player {
            silentPlayer = nil
        }
        if let error {
            logger.warning("Silent audio activation decode failed: \(error.localizedDescription)")
        }
    }
}

/// Activates the Control Center media session by emitting a short silent audio pulse.
private func activateAudioSessionWithSilence() {
    // Create a short silent audio buffer (0.1 seconds of silence)
    let sampleRate: Double = 44100
    let duration: Double = 0.1
    let numSamples = Int(sampleRate * duration)
    
    // Create silent PCM data (16-bit stereo)
    let audioData = Data(count: numSamples * 4) // 2 channels * 2 bytes per sample
    
    // Create a WAV file in memory
    let wavData = createWAVData(from: audioData, sampleRate: Int(sampleRate), channels: 2)
    do {
        silentPlayer = try AVAudioPlayer(data: wavData)
        silentPlayer?.volume = 0
        silentPlayer?.delegate = silentPlayerDelegate
        silentPlayer?.prepareToPlay()
        if silentPlayer?.play() == false {
            silentPlayer = nil
            logger.warning("Silent audio activation pulse did not start playback")
        }
    } catch {
        logger.warning("Failed to create silent audio player: \(error.localizedDescription)")
    }
}

/// Creates a WAV file data from raw PCM data
private func createWAVData(from pcmData: Data, sampleRate: Int, channels: Int) -> Data {
    let bitsPerSample = 16
    let bytesPerSample = bitsPerSample / 8
    let byteRate = sampleRate * channels * bytesPerSample
    let blockAlign = channels * bytesPerSample
    let dataSize = pcmData.count
    let fileSize = 36 + dataSize
    
    var wavData = Data(capacity: 44 + dataSize)
    
    // RIFF header
    wavData.append(contentsOf: "RIFF".utf8)
    appendLittleEndian(UInt32(fileSize), to: &wavData)
    wavData.append(contentsOf: "WAVE".utf8)
    
    // fmt chunk
    wavData.append(contentsOf: "fmt ".utf8)
    appendLittleEndian(UInt32(16), to: &wavData) // chunk size
    appendLittleEndian(UInt16(1), to: &wavData) // PCM format
    appendLittleEndian(UInt16(channels), to: &wavData)
    appendLittleEndian(UInt32(sampleRate), to: &wavData)
    appendLittleEndian(UInt32(byteRate), to: &wavData)
    appendLittleEndian(UInt16(blockAlign), to: &wavData)
    appendLittleEndian(UInt16(bitsPerSample), to: &wavData)
    
    // data chunk
    wavData.append(contentsOf: "data".utf8)
    appendLittleEndian(UInt32(dataSize), to: &wavData)
    wavData.append(pcmData)
    
    return wavData
}

private func appendLittleEndian<T: FixedWidthInteger>(_ value: T, to data: inout Data) {
    var encoded = value.littleEndian
    withUnsafeBytes(of: &encoded) { bytes in
        data.append(contentsOf: bytes)
    }
}
#endif

func media_session_set_metadata(session_id: UInt64, metadata: MediaMetadataFFI) -> MediaResultFFI {
    guard MediaSessionRegistry.shared.activate(session_id) else {
        return .UpdateFailed
    }
    var nowPlayingInfo: [String: Any] = [:]
    
    let title = metadata.title.toString()
    let artist = metadata.artist.toString()
    let album = metadata.album.toString()
    
    if !title.isEmpty {
        nowPlayingInfo[MPMediaItemPropertyTitle] = title
    }
    if !artist.isEmpty {
        nowPlayingInfo[MPMediaItemPropertyArtist] = artist
    }
    if !album.isEmpty {
        nowPlayingInfo[MPMediaItemPropertyAlbumTitle] = album
    }
    if metadata.duration_secs >= 0 {
        nowPlayingInfo[MPMediaItemPropertyPlaybackDuration] = metadata.duration_secs
    }
    
    if !metadata.artwork.isEmpty {
        var encoded = Data(capacity: Int(metadata.artwork.len()))
        for byte in metadata.artwork {
            encoded.append(byte)
        }
        guard let image = PlatformImage(data: encoded) else {
            preconditionFailure("Apple media session received invalid encoded artwork")
        }
        #if os(iOS)
        let artwork = MPMediaItemArtwork(boundsSize: image.size) { _ in image }
        #else
        let artwork = MPMediaItemArtwork(
            boundsSize: NSSize(width: image.size.width, height: image.size.height)
        ) { _ in image }
        #endif
        nowPlayingInfo[MPMediaItemPropertyArtwork] = artwork
    }
    
    MPNowPlayingInfoCenter.default().nowPlayingInfo = nowPlayingInfo
    return .Success
}

func media_session_set_playback_state(session_id: UInt64, state: PlaybackStateFFI) -> MediaResultFFI {
    guard MediaSessionRegistry.shared.activate(session_id) else {
        return .UpdateFailed
    }
    var nowPlayingInfo = MPNowPlayingInfoCenter.default().nowPlayingInfo ?? [:]
    let commandCenter = MPRemoteCommandCenter.shared()
    
    // Update position
    if state.position_secs >= 0 {
        nowPlayingInfo[MPNowPlayingInfoPropertyElapsedPlaybackTime] = state.position_secs
    }
    
    // Update rate
    nowPlayingInfo[MPNowPlayingInfoPropertyPlaybackRate] = state.rate

    commandCenter.nextTrackCommand.isEnabled = state.next_enabled
    commandCenter.previousTrackCommand.isEnabled = state.previous_enabled
    
    MPNowPlayingInfoCenter.default().nowPlayingInfo = nowPlayingInfo
    
    // Update playback state (iOS and macOS 10.12.2+)
    switch state.status {
    case 0: // Stopped
        MPNowPlayingInfoCenter.default().playbackState = .stopped
    case 1: // Paused
        MPNowPlayingInfoCenter.default().playbackState = .paused
    case 2: // Playing
        MPNowPlayingInfoCenter.default().playbackState = .playing
    default:
        break
    }
    
    return .Success
}

private func registerMediaCommandHandlers() {
    let commandCenter = MPRemoteCommandCenter.shared()
    
    // IMPORTANT: On macOS, we must explicitly enable commands for Now Playing to appear
    commandCenter.playCommand.isEnabled = true
    commandCenter.playCommand.addTarget { _ in
        MediaSessionRegistry.shared.sendToActive(.play)
        return .success
    }
    
    commandCenter.pauseCommand.isEnabled = true
    commandCenter.pauseCommand.addTarget { _ in
        MediaSessionRegistry.shared.sendToActive(.pause)
        return .success
    }
    
    commandCenter.togglePlayPauseCommand.isEnabled = true
    commandCenter.togglePlayPauseCommand.addTarget { _ in
        MediaSessionRegistry.shared.sendToActive(.playPause)
        return .success
    }
    
    commandCenter.stopCommand.isEnabled = true
    commandCenter.stopCommand.addTarget { _ in
        MediaSessionRegistry.shared.sendToActive(.stop)
        return .success
    }
    
    commandCenter.nextTrackCommand.isEnabled = true
    commandCenter.nextTrackCommand.addTarget { _ in
        MediaSessionRegistry.shared.sendToActive(.next)
        return .success
    }
    
    commandCenter.previousTrackCommand.isEnabled = true
    commandCenter.previousTrackCommand.addTarget { _ in
        MediaSessionRegistry.shared.sendToActive(.previous)
        return .success
    }
    
    commandCenter.changePlaybackPositionCommand.isEnabled = true
    commandCenter.changePlaybackPositionCommand.addTarget { event in
        if let positionEvent = event as? MPChangePlaybackPositionCommandEvent {
            MediaSessionRegistry.shared.sendToActive(.seek, valueSeconds: positionEvent.positionTime)
        }
        return .success
    }
    
    commandCenter.skipForwardCommand.isEnabled = true
    commandCenter.skipForwardCommand.preferredIntervals = [15]
    commandCenter.skipForwardCommand.addTarget { event in
        if let skipEvent = event as? MPSkipIntervalCommandEvent {
            MediaSessionRegistry.shared.sendToActive(.seekForward, valueSeconds: skipEvent.interval)
        }
        return .success
    }
    
    commandCenter.skipBackwardCommand.isEnabled = true
    commandCenter.skipBackwardCommand.preferredIntervals = [15]
    commandCenter.skipBackwardCommand.addTarget { event in
        if let skipEvent = event as? MPSkipIntervalCommandEvent {
            MediaSessionRegistry.shared.sendToActive(.seekBackward, valueSeconds: skipEvent.interval)
        }
        return .success
    }
}

func media_session_request_audio_focus(session_id: UInt64) -> MediaResultFFI {
    guard MediaSessionRegistry.shared.activate(session_id) else {
        return .UpdateFailed
    }
    #if os(iOS)
    do {
        try AVAudioSession.sharedInstance().setCategory(.playback, mode: .default)
        try AVAudioSession.sharedInstance().setActive(true)
        return .Success
    } catch {
        return .AudioFocusDenied
    }
    #else
    // macOS doesn't have audio focus in the same way
    return .Success
    #endif
}

func media_session_abandon_audio_focus(session_id: UInt64) -> MediaResultFFI {
    guard MediaSessionRegistry.shared.activate(session_id) else {
        return .UpdateFailed
    }
    #if os(iOS)
    do {
        try AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        return .Success
    } catch {
        return .UpdateFailed
    }
    #else
    return .Success
    #endif
}

#if os(iOS)
private func registerSystemEventObservers() {
    guard !systemEventObserversRegistered else { return }

    let center = NotificationCenter.default
    let audioSession = AVAudioSession.sharedInstance()
    let interruption = center.addObserver(
        forName: AVAudioSession.interruptionNotification,
        object: audioSession,
        queue: .main
    ) { notification in
        handleAudioSessionInterruption(notification)
    }
    let routeChange = center.addObserver(
        forName: AVAudioSession.routeChangeNotification,
        object: audioSession,
        queue: .main
    ) { notification in
        handleAudioSessionRouteChange(notification)
    }
    let silenceSecondaryAudioHint = center.addObserver(
        forName: AVAudioSession.silenceSecondaryAudioHintNotification,
        object: audioSession,
        queue: .main
    ) { notification in
        handleAudioSessionSilenceSecondaryAudioHint(notification)
    }

    systemEventObserverTokens = [interruption, routeChange, silenceSecondaryAudioHint]
    systemEventObserversRegistered = true
}

private func unregisterSystemEventObservers() {
    guard systemEventObserversRegistered else { return }

    let center = NotificationCenter.default
    for token in systemEventObserverTokens {
        center.removeObserver(token)
    }
    systemEventObserverTokens.removeAll()
    systemEventObserversRegistered = false
}

private func handleAudioSessionInterruption(_ notification: Notification) {
    guard
        let rawType = notification.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
        let type = AVAudioSession.InterruptionType(rawValue: rawType)
    else {
        return
    }

    switch type {
    case .began:
        MediaSessionRegistry.shared.sendToActive(.audioFocusLostTransient)
    case .ended:
        let rawOptions = notification.userInfo?[AVAudioSessionInterruptionOptionKey] as? UInt ?? 0
        let options = AVAudioSession.InterruptionOptions(rawValue: rawOptions)
        if options.contains(.shouldResume) {
            MediaSessionRegistry.shared.sendToActive(.audioFocusGained)
        } else {
            MediaSessionRegistry.shared.sendToActive(.audioFocusLost)
        }
    @unknown default:
        fatalError("Unsupported AVAudioSession interruption type")
    }
}

private func handleAudioSessionRouteChange(_ notification: Notification) {
    guard
        let rawReason = notification.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt,
        let reason = AVAudioSession.RouteChangeReason(rawValue: rawReason)
    else {
        return
    }

    switch reason {
    case .oldDeviceUnavailable:
        MediaSessionRegistry.shared.sendToActive(.audioBecomingNoisy)
    case .newDeviceAvailable,
         .categoryChange,
         .override,
         .wakeFromSleep,
         .noSuitableRouteForCategory,
         .routeConfigurationChange,
         .unknown:
        return
    @unknown default:
        fatalError("Unsupported AVAudioSession route change reason")
    }
}

private func handleAudioSessionSilenceSecondaryAudioHint(_ notification: Notification) {
    guard
        let rawType = notification.userInfo?[AVAudioSessionSilenceSecondaryAudioHintTypeKey] as? UInt,
        let type = AVAudioSession.SilenceSecondaryAudioHintType(rawValue: rawType)
    else {
        return
    }

    switch type {
    case .begin:
        MediaSessionRegistry.shared.sendToActive(.audioFocusLostDuck)
    case .end:
        MediaSessionRegistry.shared.sendToActive(.audioFocusGained)
    @unknown default:
        fatalError("Unsupported AVAudioSession silence secondary audio hint type")
    }
}
#endif

func media_session_wait_command(session_id: UInt64) -> MediaCommandFFI {
    guard let command = MediaSessionRegistry.shared.waitForCommand(sessionId: session_id) else {
        return MediaCommandFFI(kind: MediaCommandKind.none.rawValue, value_secs: 0)
    }
    return MediaCommandFFI(kind: command.kind.rawValue, value_secs: command.valueSeconds)
}

func media_session_clear(session_id: UInt64) -> MediaResultFFI {
    guard MediaSessionRegistry.shared.closeSession(session_id) else {
        return .Success
    }
    MPNowPlayingInfoCenter.default().nowPlayingInfo = nil
    #if os(iOS)
    if !MediaSessionRegistry.shared.hasSessions() {
        unregisterSystemEventObservers()
    }
    #endif
    return .Success
}

// MARK: - Helpers

#if os(iOS)
import UIKit
import AVFoundation
typealias PlatformImage = UIImage
#else
import AppKit
typealias PlatformImage = NSImage
#endif
