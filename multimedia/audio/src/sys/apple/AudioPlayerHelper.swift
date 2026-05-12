import AVFoundation
import Foundation
import MediaPlayer
import OSLog

private let audioPlayerLogger = Logger(subsystem: "dev.waterui", category: "WaterKitAudioPlayer")
private let minimumPlaybackRate: Float = 0.25

private var audioPlayer: AVPlayer?
private var requestedPlaybackRate: Float = 1.0
private var preservePitch = true
private var configuredVolume: Float = 1.0

func audio_player_init() -> PlayerResultFFI {
    #if os(iOS)
    do {
        try AVAudioSession.sharedInstance().setCategory(.playback, mode: .default)
        try AVAudioSession.sharedInstance().setActive(true)
    } catch {
        audioPlayerLogger.error("Failed to activate AVAudioSession: \(error.localizedDescription)")
        return .LoadFailed
    }
    #endif

    media_session_register_command_handler()
    return .Success
}

func audio_player_load_file(path: RustString) -> PlayerResultFFI {
    let url = URL(fileURLWithPath: path.toString())
    return loadPlayerItem(url: url)
}

func audio_player_load_url(url: RustString) -> PlayerResultFFI {
    guard let parsedURL = URL(string: url.toString()) else {
        audioPlayerLogger.error("audio_player_load_url received invalid URL")
        return .LoadFailed
    }
    return loadPlayerItem(url: parsedURL)
}

func audio_player_pause() -> PlayerResultFFI {
    guard let player = audioPlayer else {
        return .PlaybackFailed
    }

    player.pause()
    updateNowPlayingPlaybackInfo()
    return .Success
}

func audio_player_resume() -> PlayerResultFFI {
    guard let player = audioPlayer else {
        return .PlaybackFailed
    }

    player.play()
    player.rate = max(requestedPlaybackRate, minimumPlaybackRate)
    updateNowPlayingPlaybackInfo()
    return .Success
}

func audio_player_stop() -> PlayerResultFFI {
    stopCurrentPlayer()
    MPNowPlayingInfoCenter.default().nowPlayingInfo = nil
    return .Success
}

func audio_player_seek(position_secs: Double) -> PlayerResultFFI {
    guard let player = audioPlayer else {
        return .PlaybackFailed
    }

    let target = CMTime(seconds: position_secs, preferredTimescale: 1_000)
    player.seek(to: target)
    updateNowPlayingPlaybackInfo()
    return .Success
}

func audio_player_set_volume(volume: Float) -> PlayerResultFFI {
    configuredVolume = volume
    audioPlayer?.volume = volume
    return .Success
}

func audio_player_set_playback_rate(rate: Float) -> PlayerResultFFI {
    requestedPlaybackRate = rate
    guard let player = audioPlayer else {
        return .Success
    }
    if player.rate > 0 {
        player.rate = max(rate, minimumPlaybackRate)
    }
    updateNowPlayingPlaybackInfo()
    return .Success
}

func audio_player_set_preserve_pitch(preserve_pitch: Bool) -> PlayerResultFFI {
    preservePitch = preserve_pitch
    applyPitchAlgorithm()
    updateNowPlayingPlaybackInfo()
    return .Success
}

func audio_player_get_state() -> PlayerStateFFI {
    guard let player = audioPlayer else {
        return PlayerStateFFI(state: 0, position_secs: -1.0, duration_secs: -1.0)
    }

    let state: UInt8 = player.rate > 0 ? 2 : 1
    let position = player.currentTime().seconds
    let duration = currentDurationSeconds(player: player)
    return PlayerStateFFI(
        state: state,
        position_secs: position.isFinite ? position : -1.0,
        duration_secs: duration
    )
}

private func loadPlayerItem(url: URL) -> PlayerResultFFI {
    stopCurrentPlayer()

    let item = AVPlayerItem(url: url)
    item.audioTimePitchAlgorithm = preservePitch ? .spectral : .varispeed

    let player = AVPlayer(playerItem: item)
    player.volume = configuredVolume
    player.automaticallyWaitsToMinimizeStalling = true
    player.pause()
    audioPlayer = player

    updateNowPlayingPlaybackInfo()
    return .Success
}

private func stopCurrentPlayer() {
    audioPlayer?.pause()
    audioPlayer?.replaceCurrentItem(with: nil)
    audioPlayer = nil
}

private func applyPitchAlgorithm() {
    guard let item = audioPlayer?.currentItem else {
        return
    }
    item.audioTimePitchAlgorithm = preservePitch ? .spectral : .varispeed
}

private func currentDurationSeconds(player: AVPlayer) -> Double {
    guard let item = player.currentItem else {
        return -1.0
    }

    let duration = item.duration.seconds
    return duration.isFinite ? duration : -1.0
}

private func updateNowPlayingPlaybackInfo() {
    guard audioPlayer != nil else {
        MPNowPlayingInfoCenter.default().nowPlayingInfo = nil
        return
    }

    var nowPlayingInfo = MPNowPlayingInfoCenter.default().nowPlayingInfo ?? [:]
    let state = audio_player_get_state()

    if state.position_secs >= 0 {
        nowPlayingInfo[MPNowPlayingInfoPropertyElapsedPlaybackTime] = state.position_secs
    }
    if state.duration_secs >= 0 {
        nowPlayingInfo[MPMediaItemPropertyPlaybackDuration] = state.duration_secs
    }

    switch state.state {
    case 2:
        nowPlayingInfo[MPNowPlayingInfoPropertyPlaybackRate] = Double(requestedPlaybackRate)
        MPNowPlayingInfoCenter.default().playbackState = .playing
    case 1:
        nowPlayingInfo[MPNowPlayingInfoPropertyPlaybackRate] = 0.0
        MPNowPlayingInfoCenter.default().playbackState = .paused
    default:
        nowPlayingInfo[MPNowPlayingInfoPropertyPlaybackRate] = 0.0
        MPNowPlayingInfoCenter.default().playbackState = .stopped
    }

    MPNowPlayingInfoCenter.default().nowPlayingInfo = nowPlayingInfo
}
