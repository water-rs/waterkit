import Foundation
#if os(iOS)
import UIKit
#elseif os(macOS)
import AppKit
#endif

func show_printer_bridge(cb_id: UInt64) {
    DispatchQueue.main.async {
        #if os(macOS)
        let printInfo = NSPrintInfo.shared
        let printPanel = NSPrintPanel()
        
        // This is a bit unusual: runModal usually takes an NSPrintOperation
        // or uses runModalWithPrintInfo: which is deprecated/older API.
        // Modern way is runModal(with:delegate:didRun:contextInfo:) or similar for sheets,
        // but for blocking modal we can use runModal(with:).
        // However, NSPrintPanel.runModal(with:) runs the panel to modify the print info.
        
        let result = printPanel.runModal(with: printInfo)
        
        // standard result is NSApplication.ModalResponse (legacy Int)
        // NSPrintPanel.runModal returns Int (NSModalResponse in ObjC)
        
        if result == NSApplication.ModalResponse.OK.rawValue {
            on_printer_result(cb_id, true)
        } else {
            on_printer_result(cb_id, false)
        }
        
        #elseif os(iOS)
        // Printing on iOS requires UIPrintInteractionController
        let printController = UIPrintInteractionController.shared
        let printInfo = UIPrintInfo(dictionary:nil)
        printInfo.outputType = .general
        printInfo.jobName = "WaterKit Print Job"
        printController.printInfo = printInfo
        
        // We need something to print, usually. If nil, it might fail or show error.
        // But for testing/dialog trigger, we can try presenting it.
        
        // We need a view controller to present from or separate implementation.
        // Using existing helper.
        if let topVC = getTopViewController() {
             printController.present(animated: true) { (controller, completed, error) in
                 on_printer_result(cb_id, completed)
             }
        } else {
             on_printer_result(cb_id, false)
        }
        #endif
    }
}
