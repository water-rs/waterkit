import CoreFoundation
import CoreImage
import CoreVideo
import Foundation
import UIKit
import VisionKit

// The same key-window lookup the code scanner's bridge performs. All of the
// crate's Swift bridges concatenate into one file, so this copy carries a
// feature-specific name to keep it distinct from the code scanner's.
private func documentScannerTopViewController() -> UIViewController? {
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

// A page crosses the bridge as a retained `CVPixelBuffer` so Vision serves
// it without any decode or re-encode; the crate's buffers all share the
// 32BGRA IOSurface-backed layout the video player's render targets use.
private func documentScanPageBuffer(
    _ image: UIImage,
    context: CIContext
) -> CVPixelBuffer? {
    guard let cgImage = image.cgImage else { return nil }
    let width = cgImage.width
    let height = cgImage.height
    var pixelBuffer: CVPixelBuffer?
    let attributes: [CFString: Any] = [
        kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_32BGRA,
        kCVPixelBufferWidthKey: width,
        kCVPixelBufferHeightKey: height,
        kCVPixelBufferMetalCompatibilityKey: true,
        kCVPixelBufferIOSurfacePropertiesKey: [:] as CFDictionary,
    ]
    let status = CVPixelBufferCreate(
        kCFAllocatorDefault,
        width,
        height,
        kCVPixelFormatType_32BGRA,
        attributes as CFDictionary,
        &pixelBuffer
    )
    guard status == kCVReturnSuccess, let pixelBuffer else { return nil }
    context.render(
        CIImage(cgImage: cgImage),
        to: pixelBuffer,
        bounds: CGRect(x: 0, y: 0, width: width, height: height),
        colorSpace: CGColorSpaceCreateDeviceRGB()
    )
    return pixelBuffer
}

// The delegate the presented scanner keeps until the scan settles.
@MainActor
private var activeDocumentScanDelegates: Set<DocumentScanDelegate> = []

@MainActor
private func finishDocumentScan(
    delegate: DocumentScanDelegate,
    scanner: VNDocumentCameraViewController,
    pages: RustVec<UInt>,
    error: String?
) {
    activeDocumentScanDelegates.remove(delegate)
    scanner.dismiss(animated: true) {
        document_scan_reply_complete(delegate.reply, pages, error)
    }
}

@MainActor
private final class DocumentScanDelegate: NSObject, VNDocumentCameraViewControllerDelegate {
    let reply: DocumentScanReply
    private var finished = false

    init(reply: DocumentScanReply) {
        self.reply = reply
    }

    private func finish(
        _ scanner: VNDocumentCameraViewController,
        pages: RustVec<UInt> = RustVec(),
        error: String? = nil
    ) {
        guard !finished else { return }
        finished = true
        finishDocumentScan(
            delegate: self, scanner: scanner, pages: pages, error: error)
    }

    func documentCameraViewController(
        _ controller: VNDocumentCameraViewController,
        didFinishWith scan: VNDocumentCameraScan
    ) {
        guard !finished else { return }
        // Rendering a page is GPU work sized by the photo; keep it off the
        // main actor while the camera tears itself down.
        DispatchQueue.global(qos: .userInitiated).async {
            let context = CIContext()
            var buffers: [CVPixelBuffer] = []
            buffers.reserveCapacity(scan.pageCount)
            var failure: String?
            for index in 0..<scan.pageCount {
                guard let buffer = documentScanPageBuffer(
                    scan.imageOfPage(at: index), context: context)
                else {
                    failure = "scanned page \(index) did not render into a pixel buffer"
                    break
                }
                buffers.append(buffer)
            }
            let pages = RustVec<UInt>()
            if failure == nil {
                for buffer in buffers {
                    // Rust adopts this retain when it wraps the address
                    // back in a `CFRetained`.
                    pages.push(value: UInt(
                        bitPattern: Unmanaged.passRetained(buffer).toOpaque()))
                }
            }
            DispatchQueue.main.async {
                if let failure {
                    self.finish(controller, error: failure)
                } else {
                    self.finish(controller, pages: pages)
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

func scan_document_bridge(reply: DocumentScanReply) {
    DispatchQueue.main.async {
        guard VNDocumentCameraViewController.isSupported else {
            document_scan_reply_complete(
                reply, RustVec(),
                "VNDocumentCameraViewController is unsupported")
            return
        }
        guard let topVC = documentScannerTopViewController() else {
            document_scan_reply_complete(
                reply, RustVec(),
                "no key window scene to present the scanner from")
            return
        }
        let delegate = DocumentScanDelegate(reply: reply)
        let scanner = VNDocumentCameraViewController()
        scanner.delegate = delegate

        activeDocumentScanDelegates.insert(delegate)
        topVC.present(scanner, animated: true)
    }
}
