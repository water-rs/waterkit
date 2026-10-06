import Foundation
import UIKit
import Vision
import VisionKit

// Maps one vocabulary id onto `VNBarcodeSymbology`. VisionKit has no UPC-A
// symbology: a UPC-A code is an EAN-13 with a leading zero, so "upca"
// registers `.ean13` and a decoded EAN-13 whose payload carries that
// leading zero reports "upca" when UPC-A was requested. `nil` means the
// device cannot express the symbology (`.msiPlessey` needs iOS 17) or the
// id is unknown.
@available(iOS 16.0, *)
private func vnSymbology(_ id: String) -> VNBarcodeSymbology? {
    switch id {
    case "aztec": return .aztec
    case "codabar": return .codabar
    case "code39": return .code39
    case "code93": return .code93
    case "code128": return .code128
    case "datamatrix": return .dataMatrix
    case "ean8": return .ean8
    case "ean13", "upca": return .ean13
    case "gs1databar": return .gs1DataBar
    case "gs1databarexpanded": return .gs1DataBarExpanded
    case "gs1databarlimited": return .gs1DataBarLimited
    case "itf": return .i2of5
    case "itf14": return .itf14
    case "micropdf417": return .microPDF417
    case "microqr": return .microQR
    case "msiplessey":
        if #available(iOS 17.0, *) { return .msiPlessey }
        return nil
    case "pdf417": return .pdf417
    case "qr": return .qr
    case "upce": return .upce
    default: return nil
    }
}

// The Rust side filters requested symbologies through
// `symbology_supported_bridge` first, so an id that maps to `nil` here is
// a contract violation.
@available(iOS 16.0, *)
private func barcodeSymbologies(_ csv: String) -> [VNBarcodeSymbology] {
    csv.split(separator: ",").map { id in
        guard let symbology = vnSymbology(String(id)) else {
            fatalError("waterkit-vision: unsupported symbology id \(id)")
        }
        return symbology
    }
}

@available(iOS 16.0, *)
private func symbologyId(_ symbology: VNBarcodeSymbology) -> String? {
    if #available(iOS 17.0, *), symbology == .msiPlessey {
        return "msiplessey"
    }
    switch symbology {
    case .aztec: return "aztec"
    case .codabar: return "codabar"
    case .code39: return "code39"
    case .code93: return "code93"
    case .code128: return "code128"
    case .dataMatrix: return "datamatrix"
    case .ean8: return "ean8"
    case .ean13: return "ean13"
    case .gs1DataBar: return "gs1databar"
    case .gs1DataBarExpanded: return "gs1databarexpanded"
    case .gs1DataBarLimited: return "gs1databarlimited"
    case .i2of5: return "itf"
    case .itf14: return "itf14"
    case .microPDF417: return "micropdf417"
    case .microQR: return "microqr"
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
    error: String?
) {
    activeScannerDelegates.removeValue(forKey: cbId)
    scanner.stopScanning()
    scanner.dismiss(animated: true) {
        on_scan_result(cbId, payload, symbology, error)
    }
}

@available(iOS 16.0, *)
@MainActor
private class ScannerDelegate: NSObject, DataScannerViewControllerDelegate {
    let cbId: UInt64
    let requestedUpca: Bool
    weak var scanner: DataScannerViewController?
    private var finished = false

    init(cbId: UInt64, requestedUpca: Bool) {
        self.cbId = cbId
        self.requestedUpca = requestedUpca
    }

    @objc func cancelTapped() {
        guard let scanner else { return }
        finish(scanner)
    }

    private func finish(
        _ scanner: DataScannerViewController,
        payload: String? = nil,
        symbology: String? = nil,
        error: String? = nil
    ) {
        guard !finished else { return }
        finished = true
        finishScan(
            cbId: cbId, scanner: scanner, payload: payload,
            symbology: symbology, error: error)
    }

    private func accept(_ scanner: DataScannerViewController, barcode: RecognizedItem.Barcode) {
        guard var symbology = symbologyId(barcode.observation.symbology) else {
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
        // A UPC-A is an EAN-13 with a leading zero: when the request asked
        // for UPC-A, a leading-0 EAN-13 reports as "upca".
        if symbology == "ean13", requestedUpca, payload.hasPrefix("0") {
            symbology = "upca"
        }
        finish(scanner, payload: payload, symbology: symbology)
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

// Whether this device's `DataScannerViewController` can restrict a scan
// to the given vocabulary id.
func symbology_supported_bridge(id: RustStr) -> Bool {
    guard #available(iOS 16.0, *) else { return false }
    return vnSymbology(id.toString()) != nil
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
                cb_id, nil as String?, nil as String?,
                "DataScannerViewController is unsupported")
            return
        }
        guard let topVC = getTopViewController() else {
            on_scan_result(
                cb_id, nil as String?, nil as String?,
                "no key window scene to present the scanner from")
            return
        }
        let delegate = ScannerDelegate(
            cbId: cb_id,
            requestedUpca: csv.split(separator: ",").contains("upca"))
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
                    error: "start scanning: \(error.localizedDescription)")
            }
        }
    }
}
