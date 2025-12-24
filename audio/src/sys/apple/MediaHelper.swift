import Foundation
import MediaPlayer
import AVFoundation

// MARK: - Media Session State

private var commandHandlerRegistered = false
private var silentPlayer: AVAudioPlayer?

// MARK: - FFI Functions

func media_session_init() -> MediaResultFFI {
    // On macOS, we need to briefly "play" something to activate the audio session
    // so that MPNowPlayingInfoCenter shows up in Control Center
    #if os(macOS)
    activateAudioSessionWithSilence()
    #endif
    return .Success
}

#if os(macOS)
/// Activates the audio session by playing a silent audio buffer.
/// This workaround is required on macOS because MPNowPlayingInfoCenter
/// only appears in Control Center when the app has an active audio session.
private func activateAudioSessionWithSilence() {
    // Create a short silent audio buffer (0.1 seconds of silence)
    let sampleRate: Double = 44100
    let duration: Double = 0.1
    let numSamples = Int(sampleRate * duration)
    
    // Create silent PCM data (16-bit stereo)
    var audioData = Data()
    let silence = [UInt8](repeating: 0, count: numSamples * 4) // 2 channels * 2 bytes per sample
    audioData.append(contentsOf: silence)
    
    // Create a WAV file in memory
    if let wavData = createWAVData(from: audioData, sampleRate: Int(sampleRate), channels: 2) {
        do {
            silentPlayer = try AVAudioPlayer(data: wavData)
            silentPlayer?.volume = 0
            silentPlayer?.play()
            // Stop after a brief moment - the play() call is what activates the session
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) {
                silentPlayer?.stop()
            }
        } catch {
            // Silent failure - not critical if this doesn't work
            print("waterkit-media: Failed to create silent audio player: \(error)")
        }
    }
}

/// Creates a WAV file data from raw PCM data
private func createWAVData(from pcmData: Data, sampleRate: Int, channels: Int) -> Data? {
    let bitsPerSample = 16
    let bytesPerSample = bitsPerSample / 8
    let byteRate = sampleRate * channels * bytesPerSample
    let blockAlign = channels * bytesPerSample
    let dataSize = pcmData.count
    let fileSize = 36 + dataSize
    
    var wavData = Data()
    
    // RIFF header
    wavData.append(contentsOf: "RIFF".utf8)
    wavData.append(contentsOf: withUnsafeBytes(of: UInt32(fileSize).littleEndian) { Array($0) })
    wavData.append(contentsOf: "WAVE".utf8)
    
    // fmt chunk
    wavData.append(contentsOf: "fmt ".utf8)
    wavData.append(contentsOf: withUnsafeBytes(of: UInt32(16).littleEndian) { Array($0) }) // chunk size
    wavData.append(contentsOf: withUnsafeBytes(of: UInt16(1).littleEndian) { Array($0) }) // PCM format
    wavData.append(contentsOf: withUnsafeBytes(of: UInt16(channels).littleEndian) { Array($0) })
    wavData.append(contentsOf: withUnsafeBytes(of: UInt32(sampleRate).littleEndian) { Array($0) })
    wavData.append(contentsOf: withUnsafeBytes(of: UInt32(byteRate).littleEndian) { Array($0) })
    wavData.append(contentsOf: withUnsafeBytes(of: UInt16(blockAlign).littleEndian) { Array($0) })
    wavData.append(contentsOf: withUnsafeBytes(of: UInt16(bitsPerSample).littleEndian) { Array($0) })
    
    // data chunk
    wavData.append(contentsOf: "data".utf8)
    wavData.append(contentsOf: withUnsafeBytes(of: UInt32(dataSize).littleEndian) { Array($0) })
    wavData.append(pcmData)
    
    return wavData
}
#endif

func media_session_set_metadata(metadata: MediaMetadataFFI) -> MediaResultFFI {
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
    
    // Load artwork from URL if provided
    let artworkUrlString = metadata.artwork_url.toString()
    if !artworkUrlString.isEmpty, let url = URL(string: artworkUrlString) {
        loadArtwork(from: url) { image in
            if let image = image {
                var info = MPNowPlayingInfoCenter.default().nowPlayingInfo ?? [:]
                #if os(iOS)
                let artwork = MPMediaItemArtwork(boundsSize: image.size) { _ in image }
                #else
                let artwork = MPMediaItemArtwork(boundsSize: NSSize(width: image.size.width, height: image.size.height)) { _ in image }
                #endif
                info[MPMediaItemPropertyArtwork] = artwork
                MPNowPlayingInfoCenter.default().nowPlayingInfo = info
            }
        }
    }
    
    MPNowPlayingInfoCenter.default().nowPlayingInfo = nowPlayingInfo
    return .Success
}

func media_session_set_playback_state(state: PlaybackStateFFI) -> MediaResultFFI {
    var nowPlayingInfo = MPNowPlayingInfoCenter.default().nowPlayingInfo ?? [:]
    
    // Update position
    if state.position_secs >= 0 {
        nowPlayingInfo[MPNowPlayingInfoPropertyElapsedPlaybackTime] = state.position_secs
    }
    
    // Update rate
    nowPlayingInfo[MPNowPlayingInfoPropertyPlaybackRate] = state.rate
    
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

func media_session_register_command_handler() {
    guard !commandHandlerRegistered else { return }
    commandHandlerRegistered = true
    
    let commandCenter = MPRemoteCommandCenter.shared()
    
    // IMPORTANT: On macOS, we must explicitly enable commands for Now Playing to appear
    commandCenter.playCommand.isEnabled = true
    commandCenter.playCommand.addTarget { _ in
        rust_on_play()
        return .success
    }
    
    commandCenter.pauseCommand.isEnabled = true
    commandCenter.pauseCommand.addTarget { _ in
        rust_on_pause()
        return .success
    }
    
    commandCenter.togglePlayPauseCommand.isEnabled = true
    commandCenter.togglePlayPauseCommand.addTarget { _ in
        rust_on_play_pause()
        return .success
    }
    
    commandCenter.stopCommand.isEnabled = true
    commandCenter.stopCommand.addTarget { _ in
        rust_on_stop()
        return .success
    }
    
    commandCenter.nextTrackCommand.isEnabled = true
    commandCenter.nextTrackCommand.addTarget { _ in
        rust_on_next()
        return .success
    }
    
    commandCenter.previousTrackCommand.isEnabled = true
    commandCenter.previousTrackCommand.addTarget { _ in
        rust_on_previous()
        return .success
    }
    
    commandCenter.changePlaybackPositionCommand.isEnabled = true
    commandCenter.changePlaybackPositionCommand.addTarget { event in
        if let positionEvent = event as? MPChangePlaybackPositionCommandEvent {
            rust_on_seek_to(positionEvent.positionTime)
        }
        return .success
    }
    
    commandCenter.skipForwardCommand.isEnabled = true
    commandCenter.skipForwardCommand.preferredIntervals = [15]
    commandCenter.skipForwardCommand.addTarget { event in
        if let skipEvent = event as? MPSkipIntervalCommandEvent {
            rust_on_seek_forward(skipEvent.interval)
        }
        return .success
    }
    
    commandCenter.skipBackwardCommand.isEnabled = true
    commandCenter.skipBackwardCommand.preferredIntervals = [15]
    commandCenter.skipBackwardCommand.addTarget { event in
        if let skipEvent = event as? MPSkipIntervalCommandEvent {
            rust_on_seek_backward(skipEvent.interval)
        }
        return .success
    }
}

func media_session_request_audio_focus() -> MediaResultFFI {
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

func media_session_abandon_audio_focus() -> MediaResultFFI {
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

/// Run the macOS run loop for the specified duration.
/// This is required for MPRemoteCommandCenter to receive events in CLI apps.
func media_session_run_loop(duration_secs: Double) {
    // Use CFRunLoop to process events for the specified duration
    // This allows MPRemoteCommandCenter to receive and dispatch events
    CFRunLoopRunInMode(.defaultMode, duration_secs, false)
}

func media_session_clear() -> MediaResultFFI {
    MPNowPlayingInfoCenter.default().nowPlayingInfo = nil
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

private func loadArtwork(from url: URL, completion: @escaping (PlatformImage?) -> Void) {
    URLSession.shared.dataTask(with: url) { data, _, _ in
        guard let data = data else {
            DispatchQueue.main.async { completion(nil) }
            return
        }
        let image = PlatformImage(data: data)
        DispatchQueue.main.async { completion(image) }
    }.resume()
}
