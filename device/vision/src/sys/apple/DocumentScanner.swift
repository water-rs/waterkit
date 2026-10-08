import Foundation
import UIKit
import VisionKit

// Pages are photographs of documents; JPEG keeps them small enough to cross
// the bridge while preserving the detail recognition needs.
private let documentPageJPEGQuality: CGFloat = 0.9

// The same key-window lookup the code scanner's bridge performs; `private`
// keeps it file-scoped so the two bridges compile independently per
// feature.
private func getTopViewController() -> UIViewController? {
    let keyWindow = UIApplication.shared.connectedScenes
        .filter({ $0.activationState == .foregroundActive })
        .compactMap({ $0 as? UIWindowScene })
        .first?.windows
        .filter({ $0.isKeyWindow }).first

    var top = keyWindow?.rootViewController ?? UIApplication.shared.delegate?.window??.rootViewController
    while let presented = top?.presentedViewController {
        top = presented
    }
    return top
}

// The delegate the presented scanner keeps until the scan settles.
@MainActor
private var activeDocumentScanDelegates: [UInt64: DocumentScanDelegate] = [:]

@MainActor
private func finishDocumentScan(
    cbId: UInt64,
    scanner: VNDocumentCameraViewController,
    pagesJson: String?,
    error: String?
) {
    activeDocumentScanDelegates.removeValue(forKey: cbId)
    scanner.dismiss(animated: true) {
        on_document_scan_result(cbId, pagesJson, error)
    }
}

@MainActor
private final class DocumentScanDelegate: NSObject, VNDocumentCameraViewControllerDelegate {
    let cbId: UInt64
    private var finished = false

    init(cbId: UInt64) {
        self.cbId = cbId
    }

    private func finish(
        _ scanner: VNDocumentCameraViewController,
        pagesJson: String? = nil,
        error: String? = nil
    ) {
        guard !finished else { return }
        finished = true
        finishDocumentScan(
            cbId: cbId, scanner: scanner, pagesJson: pagesJson, error: error)
    }

    func documentCameraViewController(
        _ controller: VNDocumentCameraViewController,
        didFinishWith scan: VNDocumentCameraScan
    ) {
        guard !finished else { return }
        // JPEG-encoding a page is CPU work sized by the photo; keep it off
        // the main actor while the camera tears itself down.
        DispatchQueue.global(qos: .userInitiated).async {
            var pages: [String] = []
            pages.reserveCapacity(scan.pageCount)
            var failure: String?
            for index in 0..<scan.pageCount {
                guard let jpeg = scan.imageOfPage(at: index)
                    .jpegData(compressionQuality: documentPageJPEGQuality)
                else {
                    failure = "scanned page \(index) did not encode as JPEG"
                    break
                }
                pages.append(jpeg.base64EncodedString())
            }
            let pagesJson: String? = if failure == nil {
                (try? JSONSerialization.data(withJSONObject: pages))
                    .flatMap({ String(data: $0, encoding: .utf8) })
            } else {
                nil
            }
            DispatchQueue.main.async {
                if let failure {
                    self.finish(controller, error: failure)
                } else if let pagesJson {
                    self.finish(controller, pagesJson: pagesJson)
                } else {
                    self.finish(controller, error: "the scanned pages did not serialize")
                }
            }
        }
    }

    func documentCameraViewControllerDidCancel(
        _ controller: VNDocumentCameraViewController
    ) {
        finish(controller)
    }

    func documentCameraViewController(
        _ controller: VNDocumentCameraViewController,
        didFailWithError error: Error
    ) {
        finish(controller, error: "document scan failed: \(error.localizedDescription)")
    }
}

// Whether this device can present `VNDocumentCameraViewController` — false
// on the simulator and on hardware without a camera for document scanning.
func document_scanner_supported_bridge() -> Bool {
    // `VNDocumentCameraViewController.isSupported` is main-actor isolated; a
    // capabilities probe can run on any thread, so hop onto it when needed.
    if Thread.isMainThread {
        return MainActor.assumeIsolated { VNDocumentCameraViewController.isSupported }
    }
    return DispatchQueue.main.sync {
        MainActor.assumeIsolated { VNDocumentCameraViewController.isSupported }
    }
}

func scan_document_bridge(cb_id: UInt64) {
    DispatchQueue.main.async {
        guard VNDocumentCameraViewController.isSupported else {
            on_document_scan_result(
                cb_id, nil as String?,
                "VNDocumentCameraViewController is unsupported")
            return
        }
        guard let topVC = getTopViewController() else {
            on_document_scan_result(
                cb_id, nil as String?,
                "no key window scene to present the scanner from")
            return
        }
        let delegate = DocumentScanDelegate(cbId: cb_id)
        let scanner = VNDocumentCameraViewController()
        scanner.delegate = delegate

        activeDocumentScanDelegates[cb_id] = delegate
        topVC.present(scanner, animated: true)
    }
}
