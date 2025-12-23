import Foundation

#if os(iOS)
import UIKit
#elseif os(macOS)
import AppKit
#endif

func trigger_haptic(style: HapticFeedbackType) {
    #if os(iOS)
    switch style {
    case .Light:
        let generator = UIImpactFeedbackGenerator(style: .light)
        generator.prepare()
        generator.impactOccurred()
    case .Medium:
        let generator = UIImpactFeedbackGenerator(style: .medium)
        generator.prepare()
        generator.impactOccurred()
    case .Heavy:
        let generator = UIImpactFeedbackGenerator(style: .heavy)
        generator.prepare()
        generator.impactOccurred()
    case .Rigid:
        if #available(iOS 13.0, *) {
            let generator = UIImpactFeedbackGenerator(style: .rigid)
            generator.prepare()
            generator.impactOccurred()
        } else {
            let generator = UIImpactFeedbackGenerator(style: .medium)
            generator.prepare()
            generator.impactOccurred()
        }
    case .Soft:
        if #available(iOS 13.0, *) {
            let generator = UIImpactFeedbackGenerator(style: .soft)
            generator.prepare()
            generator.impactOccurred()
        } else {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.prepare()
            generator.impactOccurred()
        }
    case .Selection:
        let generator = UISelectionFeedbackGenerator()
        generator.prepare()
        generator.selectionChanged()
    case .Success:
        let generator = UINotificationFeedbackGenerator()
        generator.prepare()
        generator.notificationOccurred(.success)
    case .Warning:
        let generator = UINotificationFeedbackGenerator()
        generator.prepare()
        generator.notificationOccurred(.warning)
    case .Error:
        let generator = UINotificationFeedbackGenerator()
        generator.prepare()
        generator.notificationOccurred(.error)
    }
    #elseif os(macOS)
    let manager = NSHapticFeedbackManager.defaultPerformer
    let pattern: NSHapticFeedbackManager.FeedbackPattern
    
    switch style {
    case .Success, .Warning, .Error:
        pattern = .generic
    default:
        pattern = .alignment
    }
    
    manager.perform(pattern, performanceTime: .default)
    #endif
}
