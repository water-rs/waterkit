import Foundation
import MediaPlayer

// MARK: - Media Session State

private var commandHandlerRegistered = false

// MARK: - FFI Functions

func media_session_init() -> MediaResultFFI {
    // MPNowPlayingInfoCenter doesn't require explicit initialization
    return .Success
}

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
    
    // Update playback state (iOS only)
    #if os(iOS)
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
    #endif
    
    return .Success
}

func media_session_register_command_handler() {
    guard !commandHandlerRegistered else { return }
    commandHandlerRegistered = true
    
    let commandCenter = MPRemoteCommandCenter.shared()
    
    commandCenter.playCommand.addTarget { _ in
        rust_on_play()
        return .success
    }
    
    commandCenter.pauseCommand.addTarget { _ in
        rust_on_pause()
        return .success
    }
    
    commandCenter.togglePlayPauseCommand.addTarget { _ in
        rust_on_play_pause()
        return .success
    }
    
    commandCenter.stopCommand.addTarget { _ in
        rust_on_stop()
        return .success
    }
    
    commandCenter.nextTrackCommand.addTarget { _ in
        rust_on_next()
        return .success
    }
    
    commandCenter.previousTrackCommand.addTarget { _ in
        rust_on_previous()
        return .success
    }
    
    commandCenter.changePlaybackPositionCommand.addTarget { event in
        if let positionEvent = event as? MPChangePlaybackPositionCommandEvent {
            rust_on_seek_to(positionEvent.positionTime)
        }
        return .success
    }
    
    commandCenter.skipForwardCommand.addTarget { event in
        if let skipEvent = event as? MPSkipIntervalCommandEvent {
            rust_on_seek_forward(skipEvent.interval)
        }
        return .success
    }
    
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
