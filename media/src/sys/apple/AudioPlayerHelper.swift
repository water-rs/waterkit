import Foundation
import AVFoundation
import MediaPlayer

// MARK: - Audio Player State

private var audioPlayer: AVPlayer?
private var audioFilePlayer: AVAudioPlayer?
private var currentPlayerType: PlayerType = .none

private enum PlayerType {
    case none
    case avPlayer       // For URLs and streaming
    case avAudioPlayer  // For local files
}

// MARK: - FFI Functions

func audio_player_init() -> PlayerResultFFI {
    // Initialize audio session for playback
    #if os(iOS)
    do {
        try AVAudioSession.sharedInstance().setCategory(.playback, mode: .default)
        try AVAudioSession.sharedInstance().setActive(true)
    } catch {
        return .LoadFailed
    }
    #endif
    
    // Initialize command handlers if not already done
    media_session_register_command_handler()
    
    return .Success
}

func audio_player_play_file(path: RustString) -> PlayerResultFFI {
    let pathString = path.toString()
    let url = URL(fileURLWithPath: pathString)
    
    // Stop any existing playback
    stopCurrentPlayer()
    
    do {
        audioFilePlayer = try AVAudioPlayer(contentsOf: url)
        audioFilePlayer?.prepareToPlay()
        audioFilePlayer?.play()
        currentPlayerType = .avAudioPlayer
        
        // Update Now Playing info
        updateNowPlayingPlaybackInfo()
        
        return .Success
    } catch {
        print("waterkit-media: Failed to load audio file: \(error)")
        return .LoadFailed
    }
}

func audio_player_play_url(url: RustString) -> PlayerResultFFI {
    let urlString = url.toString()
    guard let audioUrl = URL(string: urlString) else {
        return .LoadFailed
    }
    
    // Stop any existing playback
    stopCurrentPlayer()
    
    let playerItem = AVPlayerItem(url: audioUrl)
    audioPlayer = AVPlayer(playerItem: playerItem)
    audioPlayer?.play()
    currentPlayerType = .avPlayer
    
    // Update Now Playing info
    updateNowPlayingPlaybackInfo()
    
    return .Success
}

func audio_player_pause() -> PlayerResultFFI {
    switch currentPlayerType {
    case .avPlayer:
        audioPlayer?.pause()
    case .avAudioPlayer:
        audioFilePlayer?.pause()
    case .none:
        return .PlaybackFailed
    }
    updateNowPlayingPlaybackInfo()
    return .Success
}

func audio_player_resume() -> PlayerResultFFI {
    switch currentPlayerType {
    case .avPlayer:
        audioPlayer?.play()
    case .avAudioPlayer:
        audioFilePlayer?.play()
    case .none:
        return .PlaybackFailed
    }
    updateNowPlayingPlaybackInfo()
    return .Success
}

func audio_player_stop() -> PlayerResultFFI {
    stopCurrentPlayer()
    MPNowPlayingInfoCenter.default().nowPlayingInfo = nil
    return .Success
}

func audio_player_seek(position_secs: Double) -> PlayerResultFFI {
    let time = CMTime(seconds: position_secs, preferredTimescale: 1000)
    
    switch currentPlayerType {
    case .avPlayer:
        audioPlayer?.seek(to: time)
    case .avAudioPlayer:
        audioFilePlayer?.currentTime = position_secs
    case .none:
        return .PlaybackFailed
    }
    updateNowPlayingPlaybackInfo()
    return .Success
}

func audio_player_set_volume(volume: Float) -> PlayerResultFFI {
    switch currentPlayerType {
    case .avPlayer:
        audioPlayer?.volume = volume
    case .avAudioPlayer:
        audioFilePlayer?.volume = volume
    case .none:
        return .PlaybackFailed
    }
    return .Success
}

func audio_player_get_state() -> PlayerStateFFI {
    var state: UInt8 = 0  // Stopped
    var position: Double = -1.0
    var duration: Double = -1.0
    
    switch currentPlayerType {
    case .avPlayer:
        if let player = audioPlayer {
            // Check if playing
            if player.rate > 0 {
                state = 2  // Playing
            } else if player.currentItem != nil {
                state = 1  // Paused
            }
            
            position = player.currentTime().seconds
            if let item = player.currentItem {
                let itemDuration = item.duration.seconds
                if !itemDuration.isNaN && !itemDuration.isInfinite {
                    duration = itemDuration
                }
            }
        }
        
    case .avAudioPlayer:
        if let player = audioFilePlayer {
            if player.isPlaying {
                state = 2  // Playing
            } else {
                state = 1  // Paused
            }
            position = player.currentTime
            duration = player.duration
        }
        
    case .none:
        break
    }
    
    return PlayerStateFFI(state: state, position_secs: position, duration_secs: duration)
}

// MARK: - Helpers

private func stopCurrentPlayer() {
    switch currentPlayerType {
    case .avPlayer:
        audioPlayer?.pause()
        audioPlayer = nil
    case .avAudioPlayer:
        audioFilePlayer?.stop()
        audioFilePlayer = nil
    case .none:
        break
    }
    currentPlayerType = .none
}

private func updateNowPlayingPlaybackInfo() {
    var nowPlayingInfo = MPNowPlayingInfoCenter.default().nowPlayingInfo ?? [:]
    
    let state = audio_player_get_state()
    
    if state.position_secs >= 0 {
        nowPlayingInfo[MPNowPlayingInfoPropertyElapsedPlaybackTime] = state.position_secs
    }
    if state.duration_secs >= 0 {
        nowPlayingInfo[MPMediaItemPropertyPlaybackDuration] = state.duration_secs
    }
    
    // Set playback rate based on state
    switch state.state {
    case 2: // Playing
        nowPlayingInfo[MPNowPlayingInfoPropertyPlaybackRate] = 1.0
        MPNowPlayingInfoCenter.default().playbackState = .playing
    case 1: // Paused
        nowPlayingInfo[MPNowPlayingInfoPropertyPlaybackRate] = 0.0
        MPNowPlayingInfoCenter.default().playbackState = .paused
    default: // Stopped
        nowPlayingInfo[MPNowPlayingInfoPropertyPlaybackRate] = 0.0
        MPNowPlayingInfoCenter.default().playbackState = .stopped
    }
    
    MPNowPlayingInfoCenter.default().nowPlayingInfo = nowPlayingInfo
}
