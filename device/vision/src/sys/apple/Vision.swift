import CoreVideo
import CoreImage
import Foundation
import ImageIO
import Metal
import Vision

// The crate's canonical symbology names map onto the Vision symbologies that
// serve each one: UPC-A detects through EAN-13, which is how Vision reports
// it.
@available(iOS 18.0, macOS 15.0, *)
private let symbologyByName: [String: [BarcodeSymbology]] = [
    "aztec": [.aztec],
    "codabar": [.codabar],
    "code-39": [.code39],
    "code-93": [.code93],
    "code-128": [.code128],
    "data-matrix": [.dataMatrix],
    "ean-8": [.ean8],
    "ean-13": [.ean13],
    "gs1-databar": [.gs1DataBar],
    "gs1-databar-expanded": [.gs1DataBarExpanded],
    "gs1-databar-limited": [.gs1DataBarLimited],
    "itf": [.i2of5, .i2of5Checksum],
    "itf-14": [.itf14],
    "micro-pdf-417": [.microPDF417],
    "micro-qr": [.microQR],
    "msi-plessey": [.msiPlessey],
    "pdf-417": [.pdf417],
    "qr": [.qr],
    "upc-a": [.ean13],
    "upc-e": [.upce],
]

// Reported observations name the symbology Vision detected: EAN-13, never
// the UPC-A alias.
@available(iOS 18.0, macOS 15.0, *)
private let symbologyName: [BarcodeSymbology: String] = [
    .aztec: "aztec",
    .codabar: "codabar",
    .code39: "code-39",
    .code93: "code-93",
    .code128: "code-128",
    .dataMatrix: "data-matrix",
    .ean8: "ean-8",
    .ean13: "ean-13",
    .gs1DataBar: "gs1-databar",
    .gs1DataBarExpanded: "gs1-databar-expanded",
    .gs1DataBarLimited: "gs1-databar-limited",
    .i2of5: "itf",
    .i2of5Checksum: "itf",
    .itf14: "itf-14",
    .microPDF417: "micro-pdf-417",
    .microQR: "micro-qr",
    .msiPlessey: "msi-plessey",
    .pdf417: "pdf-417",
    .qr: "qr",
    .upce: "upc-e",
]

private struct Outcome<Result: Encodable>: Encodable {
    let results: [Result]?
    let error: String?

    static func served(_ results: [Result]) -> Self {
        Self(results: results, error: nil)
    }

    static func failed(_ message: String) -> Self {
        Self(results: nil, error: message)
    }
}

private struct BarcodeJson: Encodable {
    let symbology: String
    let payload: [UInt8]
    let corners: [Float]
}

private struct TextLineJson: Encodable {
    let text: String
    let confidence: Float
    let corners: [Float]
    let words: [TextWordJson]
}

private struct TextWordJson: Encodable {
    let text: String
    let confidence: Float
    let corners: [Float]
}

private func json<T: Encodable>(_ value: T) -> RustString {
    let data = (try? JSONEncoder().encode(value)) ?? Data()
    return String(decoding: data, as: UTF8.self).intoRustString()
}

private func cgOrientation(_ raw: UInt8) -> CGImagePropertyOrientation? {
    raw == 0 ? nil : CGImagePropertyOrientation(rawValue: UInt32(raw))
}

@available(iOS 18.0, macOS 15.0, *)
private func uprightCorners(
    _ tl: NormalizedPoint,
    _ tr: NormalizedPoint,
    _ br: NormalizedPoint,
    _ bl: NormalizedPoint
) -> [Float] {
    // Vision normalizes from the bottom-left; the crate's corners read
    // top-left origin, so y is flipped on the way out.
    [
        Float(tl.x), Float(1 - tl.y),
        Float(tr.x), Float(1 - tr.y),
        Float(br.x), Float(1 - br.y),
        Float(bl.x), Float(1 - bl.y),
    ]
}

@available(iOS 18.0, macOS 15.0, *)
private func retain(_ handler: ImageRequestHandler) -> UInt {
    UInt(bitPattern: Unmanaged.passRetained(handler).toOpaque())
}

public func vision_supported_symbologies() -> RustString {
    guard #available(iOS 26.0, macOS 26.0, *) else {
        return "[]".intoRustString()
    }
    let supported = Set(DetectBarcodesRequest().supportedSymbologies)
    let names = symbologyByName.compactMap { (name: String, vision: [BarcodeSymbology]) -> String? in
        vision.allSatisfy(supported.contains) ? name : nil
    }.sorted()
    return json(names)
}

public func vision_supported_text_languages(level: UInt8) -> RustString {
    guard #available(iOS 26.0, macOS 26.0, *) else {
        return "[]".intoRustString()
    }
    var request = RecognizeTextRequest()
    request.recognitionLevel = level == 0 ? .fast : .accurate
    return json(request.supportedRecognitionLanguages.map(bcp47))
}

@available(iOS 16.0, macOS 13.0, *)
private func bcp47(_ language: Locale.Language) -> String {
    var parts: [String] = []
    if let code = language.languageCode {
        parts.append(code.identifier)
    }
    if let script = language.script {
        parts.append(script.identifier)
    }
    if let region = language.region {
        parts.append(region.identifier)
    }
    return parts.joined(separator: "-")
}

public func vision_handler_pixel_buffer(buffer: UInt, orientation: UInt8) -> UInt {
    guard #available(iOS 26.0, macOS 26.0, *),
        let pointer = UnsafeRawPointer(bitPattern: buffer)
    else { return 0 }
    let pixelBuffer = Unmanaged<CVPixelBuffer>.fromOpaque(pointer).takeUnretainedValue()
    return retain(ImageRequestHandler(pixelBuffer, orientation: cgOrientation(orientation)))
}

public func vision_handler_metal_texture(texture: UInt, orientation: UInt8) -> UInt {
    guard #available(iOS 26.0, macOS 26.0, *),
        let pointer = UnsafeRawPointer(bitPattern: texture)
    else { return 0 }
    let object = Unmanaged<AnyObject>.fromOpaque(pointer).takeUnretainedValue()
    guard let metalTexture = object as? MTLTexture,
        metalTexture.usage.contains(.shaderRead),
        let unflipped = CIImage(mtlTexture: metalTexture, options: nil)
    else { return 0 }
    // `CIImage(mtlTexture:)` treats storage bottom-up while a Metal texture's
    // first row is the visual top: orient the image by a vertical flip so the
    // EXIF parameter reads the texture's stored pixels exactly.
    let image = unflipped.oriented(forExifOrientation: 4)
    return retain(ImageRequestHandler(image, orientation: cgOrientation(orientation)))
}

public func vision_handler_data(data: RustVec<UInt8>, orientation: UInt8) -> UInt {
    guard #available(iOS 26.0, macOS 26.0, *) else { return 0 }
    return retain(ImageRequestHandler(Data(data), orientation: cgOrientation(orientation)))
}

public func vision_handler_release(handler: UInt) {
    guard let pointer = UnsafeRawPointer(bitPattern: handler) else { return }
    Unmanaged<AnyObject>.fromOpaque(pointer).release()
}

public func vision_detect_barcodes(
    handler: UInt,
    symbologies: RustStr,
    callback: @escaping (RustString) -> ()
) {
    guard #available(iOS 26.0, macOS 26.0, *) else {
        callback(json(Outcome<BarcodeJson>.failed("Apple Vision requires iOS 26 or macOS 26")))
        return
    }
    guard let pointer = UnsafeRawPointer(bitPattern: handler) else {
        callback(json(Outcome<BarcodeJson>.failed("the image handler was already released")))
        return
    }
    let names =
        (try? JSONDecoder().decode([String].self, from: Data(symbologies.toString().utf8))) ?? []
    var request = DetectBarcodesRequest()
    request.symbologies = Array(Set(names.flatMap { symbologyByName[$0] ?? [] }))
    let requestHandler = Unmanaged<AnyObject>.fromOpaque(pointer).takeUnretainedValue()
        as! ImageRequestHandler
    Task {
        do {
            let observations = try await requestHandler.perform(request)
            callback(
                json(
                    Outcome.served(
                        observations.map { observation in
                            // `payloadString` is the decoded text; `payloadData` is
                            // the raw codewords a binary payload needs.
                            BarcodeJson(
                                symbology: symbologyName[observation.symbology] ?? "unknown",
                                payload: Array(
                                    observation.payloadString.map { Data($0.utf8) }
                                        ?? observation.payloadData
                                        ?? Data()),
                                corners: uprightCorners(
                                    observation.topLeft, observation.topRight,
                                    observation.bottomRight, observation.bottomLeft)
                            )
                        })))
        } catch {
            callback(json(Outcome<BarcodeJson>.failed(error.localizedDescription)))
        }
    }
}

public func vision_recognize_text(
    handler: UInt,
    level: UInt8,
    languages: RustStr,
    callback: @escaping (RustString) -> ()
) {
    guard #available(iOS 26.0, macOS 26.0, *) else {
        callback(json(Outcome<TextLineJson>.failed("Apple Vision requires iOS 26 or macOS 26")))
        return
    }
    guard let pointer = UnsafeRawPointer(bitPattern: handler) else {
        callback(json(Outcome<TextLineJson>.failed("the image handler was already released")))
        return
    }
    let tags = (try? JSONDecoder().decode([String].self, from: Data(languages.toString().utf8))) ?? []
    var request = RecognizeTextRequest()
    request.recognitionLevel = level == 0 ? .fast : .accurate
    request.automaticallyDetectsLanguage = tags.isEmpty
    if !tags.isEmpty {
        request.recognitionLanguages = tags.map { Locale.Language(identifier: $0) }
    }
    let requestHandler = Unmanaged<AnyObject>.fromOpaque(pointer).takeUnretainedValue()
        as! ImageRequestHandler
    Task {
        do {
            let observations = try await requestHandler.perform(request)
            callback(
                json(
                    Outcome.served(
                        observations.map { observation in
                            let candidate = observation.topCandidates(1).first
                            return TextLineJson(
                                text: candidate?.string ?? observation.transcript,
                                confidence: candidate?.confidence ?? observation.confidence,
                                corners: uprightCorners(
                                    observation.topLeft, observation.topRight,
                                    observation.bottomRight, observation.bottomLeft),
                                words: candidate.map(words) ?? []
                            )
                        })))
        } catch {
            callback(json(Outcome<TextLineJson>.failed(error.localizedDescription)))
        }
    }
}

@available(iOS 18.0, macOS 15.0, *)
private func words(_ candidate: RecognizedText) -> [TextWordJson] {
    let string = candidate.string
    var words: [TextWordJson] = []
    var start = string.startIndex
    for index in string.indices where string[index].isWhitespace {
        if start < index {
            appendWord(&words, candidate, start..<index)
        }
        start = string.index(after: index)
    }
    if start < string.endIndex {
        appendWord(&words, candidate, start..<string.endIndex)
    }
    return words
}

@available(iOS 18.0, macOS 15.0, *)
private func appendWord(
    _ words: inout [TextWordJson],
    _ candidate: RecognizedText,
    _ range: Range<String.Index>
) {
    guard let box = candidate.boundingBox(for: range) else { return }
    words.append(
        TextWordJson(
            text: String(candidate.string[range]),
            confidence: candidate.confidence,
            corners: uprightCorners(box.topLeft, box.topRight, box.bottomRight, box.bottomLeft)
        ))
}
