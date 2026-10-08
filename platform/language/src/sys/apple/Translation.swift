import Foundation
import Translation

private struct WireError: Encodable {
    let kind: String
    let message: String?
}

private struct WirePair: Encodable {
    let source: String
    let target: String
    let status: String?

    private enum CodingKeys: String, CodingKey {
        case source
        case target
        case status
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(source, forKey: .source)
        try container.encode(target, forKey: .target)
        try container.encode(status, forKey: .status)
    }
}

private struct WireCapabilities: Encodable {
    let pairs: [WirePair]
}

private struct WireErrorReply: Encodable {
    let error: WireError
}

private struct WireReply<Payload: Encodable>: Encodable {
    private let payload: Payload

    private enum CodingKeys: String, CodingKey {
        case ok
    }

    init(ok payload: Payload) {
        self.payload = payload
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(payload, forKey: .ok)
    }
}

private enum CapabilityProbe {
    case pair(WirePair)
    case omitted
    case failure(String)
}

private enum CapabilityResult {
    case success(WireCapabilities)
    case failure(String)
}

private final class ResponseCallback: @unchecked Sendable {
    private let callback: (String) -> Void

    init(_ callback: @escaping (String) -> Void) {
        self.callback = callback
    }

    func call(_ response: String) {
        callback(response)
    }
}

private func encoded<Payload: Encodable>(_ reply: WireReply<Payload>) -> String {
    do {
        let data = try JSONEncoder().encode(reply)
        guard let value = String(data: data, encoding: .utf8) else {
            return errorReply("platform", "translation response was not UTF-8")
        }
        return value
    } catch {
        return errorReply("platform", error.localizedDescription)
    }
}

private func errorReply(_ kind: String, _ message: String) -> String {
    guard let data = try? JSONEncoder().encode(
        WireErrorReply(error: WireError(kind: kind, message: message))
    ) else {
        return ""
    }
    return String(data: data, encoding: .utf8) ?? ""
}

@available(iOS 26.0, macOS 26.0, *)
private func platformError(_ error: Error) -> String {
    if TranslationError.notInstalled ~= error {
        return errorReply("needs_download", error.localizedDescription)
    }
    if TranslationError.unsupportedSourceLanguage ~= error
        || TranslationError.unsupportedTargetLanguage ~= error
        || TranslationError.unsupportedLanguagePairing ~= error
    {
        return errorReply("unsupported_pair", error.localizedDescription)
    }
    return errorReply("platform", error.localizedDescription)
}

@available(iOS 26.0, macOS 26.0, *)
private func capabilityPairs() async -> CapabilityResult {
    let availability = LanguageAvailability()
    let languages = await availability.supportedLanguages
    let probes = await withTaskGroup(of: CapabilityProbe.self) { group in
        for source in languages {
            for target in languages where source != target {
                group.addTask {
                    let status = await availability.status(from: source, to: target)
                    switch status {
                    case .installed:
                        return .pair(
                            WirePair(
                                source: source.minimalIdentifier,
                                target: target.minimalIdentifier,
                                status: "installed"
                            )
                        )
                    case .supported:
                        return .pair(
                            WirePair(
                                source: source.minimalIdentifier,
                                target: target.minimalIdentifier,
                                status: "needs_download"
                            )
                        )
                    case .unsupported:
                        return .omitted
                    @unknown default:
                        return .failure("unknown Apple language-availability status")
                    }
                }
            }
        }

        var results: [CapabilityProbe] = []
        for await result in group {
            results.append(result)
        }
        return results
    }

    if let failure = probes.compactMap({ probe -> String? in
        if case let .failure(message) = probe {
            return message
        }
        return nil
    }).first {
        return .failure(failure)
    }

    let pairs = probes.compactMap { probe -> WirePair? in
        if case let .pair(pair) = probe {
            return pair
        }
        return nil
    }
    return .success(WireCapabilities(pairs: pairs))
}

@available(iOS 26.0, macOS 26.0, *)
private func queryCapabilities() async -> String {
    switch await capabilityPairs() {
    case let .success(capabilities):
        return encoded(WireReply(ok: capabilities))
    case let .failure(message):
        return errorReply("platform", message)
    }
}

@available(iOS 26.0, macOS 26.0, *)
private func pairStatus(source: String, target: String) async -> String {
    let availability = LanguageAvailability()
    let sourceLanguage = Locale.Language(identifier: source)
    let targetLanguage = Locale.Language(identifier: target)
    let status = await availability.status(from: sourceLanguage, to: targetLanguage)
    switch status {
    case .installed:
        return encoded(WireReply(ok: Optional("installed")))
    case .supported:
        return encoded(WireReply(ok: Optional("needs_download")))
    case .unsupported:
        return encoded(WireReply(ok: Optional<String>.none))
    @unknown default:
        return errorReply("platform", "unknown Apple language-availability status")
    }
}

@available(iOS 26.0, macOS 26.0, *)
private final class TranslationSessionRegistry: @unchecked Sendable {
    static let shared = TranslationSessionRegistry()

    private let lock = NSLock()
    private var sessions: [UInt64: TranslationSession] = [:]
    private var nextID: UInt64 = 1

    func store(source: String, target: String) -> UInt64 {
        let session = TranslationSession(
            installedSource: Locale.Language(identifier: source),
            target: Locale.Language(identifier: target)
        )
        lock.lock()
        defer { lock.unlock() }
        let id = nextID
        nextID += 1
        sessions[id] = session
        return id
    }

    func session(for id: UInt64) -> TranslationSession? {
        lock.lock()
        defer { lock.unlock() }
        return sessions[id]
    }

    func remove(id: UInt64) {
        lock.lock()
        defer { lock.unlock() }
        sessions.removeValue(forKey: id)
    }
}

private func parseStrings(_ json: String) throws -> [String] {
    guard let data = json.data(using: .utf8) else {
        throw NSError(
            domain: "waterkit.language",
            code: 1,
            userInfo: [NSLocalizedDescriptionKey: "translation request was not UTF-8"]
        )
    }
    return try JSONDecoder().decode([String].self, from: data)
}

private func encodeStrings(_ texts: [String]) -> String {
    encoded(WireReply(ok: texts))
}

@available(iOS 26.0, macOS 26.0, *)
private func translate(id: UInt64, textsJSON: String) async -> String {
    guard let session = TranslationSessionRegistry.shared.session(for: id) else {
        return errorReply("platform", "translation session \(id) was released")
    }
    do {
        let texts = try parseStrings(textsJSON)
        let requests = texts.enumerated().map { index, text in
            TranslationSession.Request(sourceText: text, clientIdentifier: String(index))
        }
        let responses = try await session.translations(from: requests)
        var ordered = [String?](repeating: nil, count: texts.count)
        for response in responses {
            guard let clientIdentifier = response.clientIdentifier,
                  let index = Int(clientIdentifier),
                  ordered.indices.contains(index),
                  ordered[index] == nil
            else {
                return errorReply("platform", "translation response has a missing or duplicate client identifier")
            }
            ordered[index] = response.targetText
        }
        guard ordered.allSatisfy({ $0 != nil }) else {
            return errorReply("platform", "translation response omitted a requested result")
        }
        return encodeStrings(ordered.compactMap { $0 })
    } catch {
        return platformError(error)
    }
}

func language_capabilities(callback: @escaping (String) -> Void) {
    guard #available(iOS 26.0, macOS 26.0, *) else {
        callback(errorReply("unavailable", "iOS 26 or macOS 26 is required"))
        return
    }
    let callback = ResponseCallback(callback)
    Task {
        callback.call(await queryCapabilities())
    }
}

func language_pair_status(
    source: RustStr,
    target: RustStr,
    callback: @escaping (String) -> Void
) {
    guard #available(iOS 26.0, macOS 26.0, *) else {
        callback(errorReply("unavailable", "iOS 26 or macOS 26 is required"))
        return
    }
    let source = source.toString()
    let target = target.toString()
    let callback = ResponseCallback(callback)
    Task {
        callback.call(await pairStatus(source: source, target: target))
    }
}

func language_translator_create(
    source: RustStr,
    target: RustStr,
    callback: @escaping (String) -> Void
) {
    guard #available(iOS 26.0, macOS 26.0, *) else {
        callback(errorReply("unavailable", "iOS 26 or macOS 26 is required"))
        return
    }
    let source = source.toString()
    let target = target.toString()
    callback(encoded(WireReply(ok: TranslationSessionRegistry.shared.store(
        source: source,
        target: target
    ))))
}

func language_translate(
    id: UInt64,
    texts_json: RustStr,
    callback: @escaping (String) -> Void
) {
    guard #available(iOS 26.0, macOS 26.0, *) else {
        callback(errorReply("unavailable", "iOS 26 or macOS 26 is required"))
        return
    }
    let textsJSON = texts_json.toString()
    let callback = ResponseCallback(callback)
    Task {
        callback.call(await translate(id: id, textsJSON: textsJSON))
    }
}

func language_translator_release(id: UInt64) {
    if #available(iOS 26.0, macOS 26.0, *) {
        TranslationSessionRegistry.shared.remove(id: id)
    }
}
