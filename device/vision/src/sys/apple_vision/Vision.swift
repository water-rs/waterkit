import CoreVideo
import CoreImage
import DataDetection
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

private struct DocumentJson: Encodable {
    let corners: [Float]
    let blocks: [BlockJson]
}

// A container's children encode heterogeneously, each struct carrying the
// `kind` tag the Rust wire enum dispatches on.
private enum BlockJson: Encodable {
    case paragraph(ParagraphJson)
    case table(TableJson)
    case list(ListJson)
    case barcode(BarcodeJson)

    func encode(to encoder: any Encoder) throws {
        switch self {
        case .paragraph(let value): try value.encode(to: encoder)
        case .table(let value): try value.encode(to: encoder)
        case .list(let value): try value.encode(to: encoder)
        case .barcode(let value): try value.encode(to: encoder)
        }
    }
}

private struct ParagraphJson: Encodable {
    let kind = "paragraph"
    let text: String
    let lines: [TextLineJson]
    let data: [DetectedDataJson]
    let corners: [Float]
}

private struct TableJson: Encodable {
    let kind = "table"
    let rows: Int
    let columns: Int
    let cells: [TableCellJson]
    let corners: [Float]
}

private struct TableCellJson: Encodable {
    let rows: [Int]
    let columns: [Int]
    let content: [BlockJson]
    let corners: [Float]
}

private struct ListJson: Encodable {
    let kind = "list"
    let items: [ListItemJson]
    let corners: [Float]
}

private struct ListItemJson: Encodable {
    let marker: String?
    let content: [BlockJson]
    let corners: [Float]
}

private struct DetectedDataJson: Encodable {
    let kind: String
    let value: String
    let range: [Int]
    let corners: [Float]
}

// A node the bridge cannot express on the wire fails the request rather
// than dropping the node silently.
private struct DocumentMappingError: LocalizedError {
    let errorDescription: String?

    init(_ description: String) {
        errorDescription = description
    }
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

// A `BoundingRegionProviding`'s extent as upright corners; a document
// node's region reports its bounding quad.
@available(iOS 26.0, macOS 26.0, *)
private func regionCorners(_ region: NormalizedRegion) -> [Float] {
    let quad = region.boundingQuad
    return uprightCorners(quad.topLeft, quad.topRight, quad.bottomRight, quad.bottomLeft)
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

public func vision_supported_document_languages() -> RustString {
    guard #available(iOS 26.0, macOS 26.0, *) else {
        return "[]".intoRustString()
    }
    return json(RecognizeDocumentsRequest().supportedRecognitionLanguages.map(bcp47))
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
            callback(json(Outcome.served(observations.map(barcodeJson))))
        } catch {
            callback(json(Outcome<BarcodeJson>.failed(error.localizedDescription)))
        }
    }
}

// `payloadString` is the decoded text; `payloadData` is the raw codewords a
// binary payload needs.
@available(iOS 18.0, macOS 15.0, *)
private func barcodeJson(_ observation: BarcodeObservation) -> BarcodeJson {
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
            callback(json(Outcome.served(observations.map(textLineJson))))
        } catch {
            callback(json(Outcome<TextLineJson>.failed(error.localizedDescription)))
        }
    }
}

@available(iOS 26.0, macOS 26.0, *)
private func textLineJson(_ observation: RecognizedTextObservation) -> TextLineJson {
    let candidate = observation.topCandidates(1).first
    return TextLineJson(
        text: candidate?.string ?? observation.transcript,
        confidence: candidate?.confidence ?? observation.confidence,
        corners: uprightCorners(
            observation.topLeft, observation.topRight,
            observation.bottomRight, observation.bottomLeft),
        words: candidate.map(words) ?? []
    )
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
public func vision_recognize_document(
    handler: UInt,
    languages: RustStr,
    callback: @escaping (RustString) -> ()
) {
    guard #available(iOS 26.0, macOS 26.0, *) else {
        callback(json(Outcome<DocumentJson>.failed("Apple Vision requires iOS 26 or macOS 26")))
        return
    }
    guard let pointer = UnsafeRawPointer(bitPattern: handler) else {
        callback(json(Outcome<DocumentJson>.failed("the image handler was already released")))
        return
    }
    let tags = (try? JSONDecoder().decode([String].self, from: Data(languages.toString().utf8))) ?? []
    var request = RecognizeDocumentsRequest()
    request.textRecognitionOptions.automaticallyDetectLanguage = tags.isEmpty
    if !tags.isEmpty {
        request.textRecognitionOptions.recognitionLanguages = tags.map {
            Locale.Language(identifier: $0)
        }
    }
    request.barcodeDetectionOptions.enabled = true
    let requestHandler = Unmanaged<AnyObject>.fromOpaque(pointer).takeUnretainedValue()
        as! ImageRequestHandler
    Task {
        do {
            let observations = try await requestHandler.perform(request)
            callback(
                json(
                    Outcome.served(
                        try observations.map { observation in
                            DocumentJson(
                                corners: regionCorners(observation.document.boundingRegion),
                                blocks: try containerBlocks(observation.document))
                        })))
        } catch {
            callback(json(Outcome<DocumentJson>.failed(error.localizedDescription)))
        }
    }
}

// The value identifying a barcode independently of the container that
// reports it: its symbology, payload and corners are equal wherever the
// same code appears.
@available(iOS 26.0, macOS 26.0, *)
private struct BarcodeIdentity: Hashable {
    let symbology: BarcodeSymbology
    let payloadString: String?
    let payloadData: Data?
    let corners: [NormalizedPoint]

    init(_ observation: BarcodeObservation) {
        symbology = observation.symbology
        payloadString = observation.payloadString
        payloadData = observation.payloadData
        corners = [
            observation.topLeft, observation.topRight,
            observation.bottomRight, observation.bottomLeft,
        ]
    }
}

// The paragraphs' and barcodes' identities nested inside a container's
// tables and lists, gathered recursively: Vision's `paragraphs` and
// `barcodes` arrays are flat, reporting nested content again at the
// container level, so nested values filter the flat arrays. A paragraph's
// identity is its recognized lines: `Container.Text` equality also compares
// `boundingRegion`, which differs between the flat and the cell-nested
// report of one paragraph, while their `lines` are equal.
@available(iOS 26.0, macOS 26.0, *)
private func collectNested(
    _ container: DocumentObservation.Container,
    into lines: inout Set<[RecognizedTextObservation]>,
    and barcodes: inout Set<BarcodeIdentity>
) {
    for paragraph in container.paragraphs {
        lines.insert(paragraph.lines)
    }
    for barcode in container.barcodes {
        barcodes.insert(BarcodeIdentity(barcode))
    }
    for table in container.tables {
        for row in table.rows {
            for cell in row {
                collectNested(cell.content, into: &lines, and: &barcodes)
            }
        }
    }
    for list in container.lists {
        for item in list.items {
            collectNested(item.content, into: &lines, and: &barcodes)
        }
    }
}

// A `DocumentObservation.Container`'s children as blocks. Nested content
// reports both inside a table's or list's container and again in the flat
// `paragraphs`/`barcodes` arrays — and Vision can even report the same
// paragraph twice within one container — so every paragraph or barcode
// emits once, by the identity of what was recognized.
//
// Reading order is `container.text.transcript`'s order. Each text-bearing
// block locates its anchor in the transcript — a paragraph or the title by
// its transcript, a table by its first non-empty cell's text in row-major
// order, a list by its first item's `itemString` — found at or after a
// cursor that advances past each located anchor, so anchors are ordered by
// first occurrence and the walk consumes the transcript forward; a block
// whose anchor cannot be located fails the request naming it. A detected
// title is a `Container.Text` like a paragraph; it emits as one unless it
// equals a paragraph, or its text was already consumed by a located block —
// Vision reports such a title as a line-less shell marking text a paragraph
// carries. Barcodes carry no transcript text: each is inserted before the
// first located text block whose bounding region's top edge lies below the
// barcode's top edge, and appended when none does.
@available(iOS 26.0, macOS 26.0, *)
private func containerBlocks(_ container: DocumentObservation.Container) throws -> [BlockJson] {
    var seenLines = Set<[RecognizedTextObservation]>()
    var seenBarcodes = Set<BarcodeIdentity>()
    for table in container.tables {
        for row in table.rows {
            for cell in row {
                collectNested(cell.content, into: &seenLines, and: &seenBarcodes)
            }
        }
    }
    for list in container.lists {
        for item in list.items {
            collectNested(item.content, into: &seenLines, and: &seenBarcodes)
        }
    }

    let transcript = container.text.transcript
    var pending: [
        (anchor: String, name: String, isTitle: Bool, top: Double, block: BlockJson)
    ] = []
    for paragraph in container.paragraphs where seenLines.insert(paragraph.lines).inserted {
        pending.append(
            (
                paragraph.transcript, "paragraph \"\(paragraph.transcript.prefix(48))\"",
                false, paragraph.boundingRegion.boundingQuad.topLeft.y,
                .paragraph(try paragraphJson(paragraph))
            ))
    }
    for table in container.tables {
        let anchor = table.rows.lazy.flatMap { $0 }
            .map { $0.content.text.transcript }
            .first { !$0.isEmpty } ?? ""
        pending.append(
            (
                anchor, "a table", false, table.boundingRegion.boundingQuad.topLeft.y,
                .table(try tableJson(table))
            ))
    }
    for list in container.lists {
        let anchor = list.items.first?.itemString ?? ""
        pending.append(
            (
                anchor, "a list", false, list.boundingRegion.boundingQuad.topLeft.y,
                .list(try listJson(list))
            ))
    }
    // The title pends last: when it shares a transcript position with a
    // paragraph — Vision marks a paragraph's first line as the title — the
    // paragraph consumes the text and the title yields to it.
    if let title = container.title,
        !container.paragraphs.contains(title),
        seenLines.insert(title.lines).inserted
    {
        pending.append(
            (
                title.transcript, "the title", true,
                title.boundingRegion.boundingQuad.topLeft.y,
                .paragraph(try paragraphJson(title))
            ))
    }

    let firstOffset = { (anchor: String) -> Int in
        guard !anchor.isEmpty, let range = transcript.range(of: anchor) else {
            return Int.max
        }
        return transcript.distance(from: transcript.startIndex, to: range.lowerBound)
    }
    let ordered = pending.enumerated()
        .sorted { (firstOffset($0.element.anchor), $0.offset) < (firstOffset($1.element.anchor), $1.offset) }
        .map { $0.element }

    var cursor = transcript.startIndex
    var located: [(top: Double, block: BlockJson)] = []
    for entry in ordered {
        guard !entry.anchor.isEmpty,
            let range = transcript.range(of: entry.anchor, range: cursor..<transcript.endIndex)
        else {
            if entry.isTitle, transcript.range(of: entry.anchor) != nil {
                continue
            }
            throw DocumentMappingError(
                "the document transcript cannot locate \(entry.name)")
        }
        located.append((entry.top, entry.block))
        cursor = range.upperBound
    }

    for barcode in container.barcodes where seenBarcodes.insert(BarcodeIdentity(barcode)).inserted {
        let top = barcode.boundingRegion.boundingQuad.topLeft.y
        let index = located.firstIndex { $0.top < top } ?? located.count
        located.insert((top, .barcode(barcodeJson(barcode))), at: index)
    }
    return located.map { $0.block }
}

@available(iOS 26.0, macOS 26.0, *)
private func paragraphJson(_ text: DocumentObservation.Container.Text) throws -> ParagraphJson {
    ParagraphJson(
        text: text.transcript,
        lines: text.lines.map(textLineJson),
        data: try text.detectedData.map { try detectedDataJson($0, in: text.transcript) },
        corners: regionCorners(text.boundingRegion))
}

// `rows`/`columns` list a spanning cell under every row and column it
// covers; each cell encodes once, in row-major order of first appearance.
@available(iOS 26.0, macOS 26.0, *)
private func tableJson(_ table: DocumentObservation.Container.Table) throws -> TableJson {
    var seen = Set<DocumentObservation.Container.Table.Cell>()
    var cells: [TableCellJson] = []
    for row in table.rows {
        for cell in row where seen.insert(cell).inserted {
            cells.append(
                TableCellJson(
                    rows: [cell.rowRange.lowerBound, cell.rowRange.upperBound + 1],
                    columns: [cell.columnRange.lowerBound, cell.columnRange.upperBound + 1],
                    content: try containerBlocks(cell.content),
                    corners: regionCorners(cell.content.boundingRegion)))
        }
    }
    return TableJson(
        rows: table.rows.count,
        columns: table.columns.count,
        cells: cells,
        corners: regionCorners(table.boundingRegion))
}

@available(iOS 26.0, macOS 26.0, *)
private func listJson(_ list: DocumentObservation.Container.List) throws -> ListJson {
    ListJson(
        items: try list.items.map { item in
            ListItemJson(
                marker: item.markerType == nil ? nil : item.markerString,
                content: try containerBlocks(item.content),
                corners: regionCorners(item.content.boundingRegion))
        },
        corners: regionCorners(list.boundingRegion))
}

// The crate's vocabulary maps only URLs, email addresses, phone numbers and
// postal addresses; every other `DataDetector.Match` kind fails the request
// naming the kind rather than dropping the detection.
@available(iOS 26.0, macOS 26.0, *)
private func detectedDataJson(
    _ detected: DocumentObservation.Container.DataDetectorMatch,
    in transcript: String
) throws -> DetectedDataJson {
    let kind: String
    let value: String
    switch detected.match.details {
    case .link(let link):
        kind = "url"
        value = link.url.absoluteString
    case .emailAddress(let email):
        kind = "email-address"
        value = email.emailAddress
    case .phoneNumber(let phone):
        kind = "phone-number"
        value = phone.phoneNumber
    case .postalAddress(let address):
        kind = "postal-address"
        value = address.fullAddress
    default:
        throw DocumentMappingError(
            "a data-detector kind this bridge does not map: \(String(describing: detected.match.details))")
    }
    guard let range = detected.match.range else {
        throw DocumentMappingError(
            "a data-detector match of kind \(kind) carries no range into the paragraph text")
    }
    // `range` indexes the paragraph's transcript; the wire carries UTF-8
    // byte offsets.
    let start = transcript.utf8.distance(from: transcript.startIndex, to: range.lowerBound)
    let end = transcript.utf8.distance(from: transcript.startIndex, to: range.upperBound)
    return DetectedDataJson(
        kind: kind,
        value: value,
        range: [start, end],
        corners: regionCorners(detected.boundingRegion))
}