import Foundation

#if os(iOS)
import UIKit
import CoreHaptics
#elseif os(macOS)
import AppKit
#endif

#if os(iOS)
private var hapticEngine: CHHapticEngine?

private func initHapticEngine() -> Bool {
    guard CHHapticEngine.capabilitiesForHardware().supportsHaptics else {
        return false
    }
    do {
        hapticEngine = try CHHapticEngine()
        try hapticEngine?.start()
        return true
    } catch {
        return false
    }
}
#endif

func haptic_is_available() -> Bool {
    #if os(iOS)
    return CHHapticEngine.capabilitiesForHardware().supportsHaptics
    #elseif os(macOS)
    return true
    #else
    return false
    #endif
}

func haptic_impact(intensity: Float) {
    #if os(iOS)
    let style: UIImpactFeedbackGenerator.FeedbackStyle
    if intensity < 0.35 {
        style = .light
    } else if intensity < 0.65 {
        style = .medium
    } else if intensity < 0.85 {
        style = .heavy
    } else {
        style = .rigid
    }
    let generator = UIImpactFeedbackGenerator(style: style)
    generator.prepare()
    generator.impactOccurred(intensity: CGFloat(intensity))
    #elseif os(macOS)
    NSHapticFeedbackManager.defaultPerformer.perform(.alignment, performanceTime: .default)
    #endif
}

func haptic_selection() {
    #if os(iOS)
    let generator = UISelectionFeedbackGenerator()
    generator.prepare()
    generator.selectionChanged()
    #elseif os(macOS)
    NSHapticFeedbackManager.defaultPerformer.perform(.alignment, performanceTime: .default)
    #endif
}

func haptic_notification(notification_type: Int32) {
    #if os(iOS)
    let generator = UINotificationFeedbackGenerator()
    generator.prepare()
    switch notification_type {
    case 0: generator.notificationOccurred(.success)
    case 1: generator.notificationOccurred(.warning)
    case 2: generator.notificationOccurred(.error)
    default: break
    }
    #elseif os(macOS)
    NSHapticFeedbackManager.defaultPerformer.perform(.generic, performanceTime: .default)
    #endif
}

func haptic_play_pattern(timings: RustVec<Int32>, intensities: RustVec<Float>, is_pause: RustVec<Bool>) -> Bool {
    #if os(iOS)
    guard CHHapticEngine.capabilitiesForHardware().supportsHaptics else {
        return false
    }

    if hapticEngine == nil {
        if !initHapticEngine() {
            return false
        }
    }

    guard let engine = hapticEngine else {
        return false
    }

    var events: [CHHapticEvent] = []
    var currentTime: TimeInterval = 0

    for index in 0..<timings.len() {
        let vectorIndex = UInt(index)
        let duration = TimeInterval(timings.get(index: vectorIndex)!) / 1000.0
        if !is_pause.get(index: vectorIndex)! {
            let intensityParam = CHHapticEventParameter(
                parameterID: .hapticIntensity,
                value: intensities.get(index: vectorIndex)!
            )
            let sharpnessParam = CHHapticEventParameter(parameterID: .hapticSharpness, value: 0.5)
            let event = CHHapticEvent(
                eventType: .hapticContinuous,
                parameters: [intensityParam, sharpnessParam],
                relativeTime: currentTime,
                duration: duration
            )
            events.append(event)
        }
        currentTime += duration
    }

    do {
        let pattern = try CHHapticPattern(events: events, parameters: [])
        let player = try engine.makePlayer(with: pattern)
        try player.start(atTime: CHHapticTimeImmediate)
        return true
    } catch {
        return false
    }
    #else
    // macOS doesn't support Core Haptics, just do a single feedback
    NSHapticFeedbackManager.defaultPerformer.perform(.generic, performanceTime: .default)
    return true
    #endif
}
