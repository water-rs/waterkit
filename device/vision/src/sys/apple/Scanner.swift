import Foundation
import UIKit
import Vision
import VisionKit

// Maps the crate's symbology vocabulary onto `VNBarcodeSymbology`. VisionKit
// has no UPC-A symbology: a UPC-A code is an EAN-13 with a leading zero, so
// requesting "upca" registers `.ean13` and decodes report "ean13".
@available(iOS 16.0, *)
private func barcodeSymbologies(_ csv: String) -> [VNBarcodeSymbology] {
    var symbologies = Set<VNBarcodeSymbology>()
    for id in csv.split(separator: ",") {
        switch id {
        case "aztec": symbologies.insert(.aztec)
        case "codabar": symbologies.insert(.codabar)
        case "code39": symbologies.insert(.code39)
        case "code93": symbologies.insert(.code93)
        case "code128": symbologies.insert(.code128)
        case "datamatrix": symbologies.insert(.dataMatrix)
        case "ean8": symbologies.insert(.ean8)
        case "ean13", "upca": symbologies.insert(.ean13)
        case "itf": symbologies.formUnion([.itf14, .i2of5])
        case "pdf417": symbologies.insert(.pdf417)
        case "qr": symbologies.insert(.qr)
        case "upce": symbologies.insert(.upce)
        default: fatalError("waterkit-vision: unknown symbology id \(id)")
        }
    }
    return Array(symbologies)
}

@available(iOS 16.0, *)
private func symbologyId(_ symbology: VNBarcodeSymbology) -> String? {
    switch symbology {
    case .aztec: return "aztec"
    case .codabar: return "codabar"
    case .code39: return "code39"
    case .code93: return "code93"
    case .code128: return "code128"
    case .dataMatrix: return "datamatrix"
    case .ean8: return "ean8"
    case .ean13: return "ean13"
    case .itf14, .i2of5: return "itf"
    case .pdf417: return "pdf417"
    case .qr: return "qr"
    case .upce: return "upce"
    default: return nil
    }
}

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
@available(iOS 16.0, *)
private var activeScannerDelegates: [UInt64: ScannerDelegate] = [:]

@available(iOS 16.0, *)
@MainActor
private func finishScan(
    cbId: UInt64,
    scanner: DataScannerViewController,
    payload: String?,
    symbology: String?,
    bounds: String?,
    error: String?
) {
    activeScannerDelegates.removeValue(forKey: cbId)
    try? scanner.stopScanning()
    scanner.dismiss(animated: true) {
        on_scan_result(cbId, payload, symbology, bounds, error)
    }
}

@available(iOS 16.0, *)
@MainActor
private class ScannerDelegate: NSObject, DataScannerViewControllerDelegate {
    let cbId: UInt64
    weak var scanner: DataScannerViewController?
    private var finished = false

    init(cbId: UInt64) {
        self.cbId = cbId
    }

    @objc func cancelTapped() {
        guard let scanner else { return }
        finish(scanner)
    }

    private func finish(
        _ scanner: DataScannerViewController,
        payload: String? = nil,
        symbology: String? = nil,
        bounds: String? = nil,
        error: String? = nil
    ) {
        guard !finished else { return }
        finished = true
        finishScan(
            cbId: cbId, scanner: scanner, payload: payload,
            symbology: symbology, bounds: bounds, error: error)
    }

    private func accept(_ scanner: DataScannerViewController, barcode: RecognizedItem.Barcode) {
        guard let symbology = symbologyId(barcode.observation.symbology) else {
            finish(
                scanner,
                error: "the scanner returned an unrecognized symbology \(barcode.observation.symbology)")
            return
        }
        guard let payload = barcode.payloadStringValue else {
            finish(
                scanner,
                error: "the scanner returned a barcode without a decodable payload")
            return
        }
        // `RecognizedItem.Bounds` corners are normalized to the presented
        // view in top-left origin — the crate's Quad convention.
        let bounds = barcode.bounds
        let corners = [
            bounds.topLeft, bounds.topRight, bounds.bottomRight, bounds.bottomLeft,
        ]
        let boundsCsv = corners.map({ "\($0.x),\($0.y)" }).joined(separator: ",")
        finish(scanner, payload: payload, symbology: symbology, bounds: boundsCsv)
    }

    func dataScanner(
        _ dataScanner: DataScannerViewController,
        didAdd addedItems: [RecognizedItem],
        allItems: [RecognizedItem]
    ) {
        for case .barcode(let barcode) in addedItems {
            accept(dataScanner, barcode: barcode)
            return
        }
    }

    func dataScanner(
        _ dataScanner: DataScannerViewController,
        didTapOn item: RecognizedItem
    ) {
        if case .barcode(let barcode) = item {
            accept(dataScanner, barcode: barcode)
        }
    }

    func dataScanner(
        _ dataScanner: DataScannerViewController,
        becameUnavailableWithError error: DataScannerViewController.ScanningUnavailable
    ) {
        finish(dataScanner, error: "scanner became unavailable: \(error)")
    }
}

func scanner_supported_bridge() -> Bool {
    guard #available(iOS 16.0, *) else { return false }
    // `DataScannerViewController.isSupported` is main-actor isolated; a
    // capabilities probe can run on any thread, so hop onto it when needed.
    if Thread.isMainThread {
        return MainActor.assumeIsolated { DataScannerViewController.isSupported }
    }
    return DispatchQueue.main.sync {
        MainActor.assumeIsolated { DataScannerViewController.isSupported }
    }
}

func scan_bridge(symbologies_csv: RustStr, cb_id: UInt64) {
    let csv = symbologies_csv.toString()
    DispatchQueue.main.async {
        guard #available(iOS 16.0, *), DataScannerViewController.isSupported else {
            on_scan_result(
                cb_id, nil as String?, nil as String?, nil as String?,
                "DataScannerViewController is unsupported")
            return
        }
        guard let topVC = getTopViewController() else {
            on_scan_result(
                cb_id, nil as String?, nil as String?, nil as String?,
                "no key window scene to present the scanner from")
            return
        }
        let delegate = ScannerDelegate(cbId: cb_id)
        let scanner = DataScannerViewController(
            recognizedDataTypes: [.barcode(symbologies: barcodeSymbologies(csv))],
            qualityLevel: .balanced,
            recognizesMultipleItems: false,
            isHighFrameRateTrackingEnabled: false,
            isPinchToZoomEnabled: true,
            isGuidanceEnabled: true,
            isHighlightingEnabled: true
        )
        scanner.delegate = delegate
        delegate.scanner = scanner

        // `DataScannerViewController` ships no cancel affordance of its own;
        // the overlay container is where its own controls live.
        let cancel = UIButton(type: .system)
        cancel.setTitle("Cancel", for: .normal)
        cancel.titleLabel?.font = .preferredFont(forTextStyle: .body)
        cancel.addTarget(
            delegate, action: #selector(ScannerDelegate.cancelTapped),
            for: .touchUpInside)
        cancel.translatesAutoresizingMaskIntoConstraints = false
        scanner.overlayContainerView.addSubview(cancel)
        NSLayoutConstraint.activate([
            cancel.centerXAnchor.constraint(
                equalTo: scanner.overlayContainerView.centerXAnchor),
            cancel.bottomAnchor.constraint(
                equalTo: scanner.overlayContainerView.safeAreaLayoutGuide.bottomAnchor,
                constant: -16),
        ])

        activeScannerDelegates[cb_id] = delegate
        topVC.present(scanner, animated: true) {
            do {
                try scanner.startScanning()
            } catch {
                finishScan(
                    cbId: cb_id, scanner: scanner, payload: nil, symbology: nil,
                    bounds: nil,
                    error: "start scanning: \(error.localizedDescription)")
            }
        }
    }
}
