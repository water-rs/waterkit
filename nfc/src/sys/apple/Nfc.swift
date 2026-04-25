import Foundation
#if os(iOS)
import CoreNFC

private var activeSessions: [UInt64: NFCSession] = [:]

func nfc_is_available_bridge() -> Bool {
    return NFCNDEFReaderSession.readingAvailable
}

func nfc_start_session(
    message: RustStr,
    tag_ctx: UInt64,
    error_callback: @escaping (String) -> Void
) {
    let msg = message.toString()
    DispatchQueue.main.async {
        let session = NFCSession(tagCtx: tag_ctx, message: msg, errorCallback: error_callback)
        activeSessions[tag_ctx] = session
        session.start()
    }
}

func nfc_stop_session(tag_ctx: UInt64) {
    DispatchQueue.main.async {
        if let session = activeSessions.removeValue(forKey: tag_ctx) {
            session.stop()
        }
    }
}

func nfc_write_message(
    tag_ctx: UInt64,
    records_json: RustStr,
    callback: @escaping (String) -> Void
) {
    let json = records_json.toString()
    DispatchQueue.main.async {
        guard let session = activeSessions[tag_ctx] else {
            callback("No active session")
            return
        }
        session.pendingWrite = (callback, json)
    }
}

class NFCSession: NSObject, NFCNDEFReaderSessionDelegate {
    let tagCtx: UInt64
    var readerSession: NFCNDEFReaderSession?
    var errorCallback: ((String) -> Void)?
    var pendingWrite: (((String) -> Void), String)? = nil

    init(
        tagCtx: UInt64,
        message: String,
        errorCallback: @escaping (String) -> Void
    ) {
        self.tagCtx = tagCtx
        self.errorCallback = errorCallback
        super.init()
        self.readerSession = NFCNDEFReaderSession(delegate: self, queue: .main, invalidateAfterFirstRead: false)
        self.readerSession?.alertMessage = message
    }

    func start() {
        readerSession?.begin()
    }

    func stop() {
        readerSession?.invalidate()
        readerSession = nil
    }

    func readerSession(_ session: NFCNDEFReaderSession, didDetectNDEFs messages: [NFCNDEFMessage]) {
        for message in messages {
            var recordsJson = ""
            for record in message.records {
                let tnf = record.typeNameFormat.rawValue
                let typeHex = record.type.map { String(format: "%02x", $0) }.joined()
                let payloadHex = record.payload.map { String(format: "%02x", $0) }.joined()
                if !recordsJson.isEmpty { recordsJson += ";" }
                recordsJson += "\(tnf):\(typeHex):\(payloadHex)"
            }
            on_nfc_tag_discovered_raw(tagCtx, "", "4", recordsJson)
        }
    }

    func readerSession(_ session: NFCNDEFReaderSession, didDetect tags: [NFCNDEFTag]) {
        guard let tag = tags.first else { return }

        session.connect(to: tag) { error in
            if let error = error {
                self.reportSessionError(error.localizedDescription)
                return
            }

            tag.queryNDEFStatus { status, capacity, error in
                if let error = error {
                    self.reportSessionError(error.localizedDescription)
                    return
                }

                if let (writeCallback, recordsJson) = self.pendingWrite {
                    self.pendingWrite = nil
                    if status == .readWrite {
                        let records = self.parseRecords(recordsJson)
                        let ndefMessage = NFCNDEFMessage(records: records)
                        tag.writeNDEF(ndefMessage) { error in
                            if let error = error {
                                writeCallback(error.localizedDescription)
                            } else {
                                writeCallback("")
                            }
                        }
                    } else {
                        writeCallback("Tag is read-only")
                    }
                } else {
                    tag.readNDEF { message, error in
                        if let error = error {
                            self.reportSessionError(error.localizedDescription)
                            return
                        }
                        var recordsJson = ""
                        if let message = message {
                            for record in message.records {
                                let tnf = record.typeNameFormat.rawValue
                                let typeHex = record.type.map { String(format: "%02x", $0) }.joined()
                                let payloadHex = record.payload.map { String(format: "%02x", $0) }.joined()
                                if !recordsJson.isEmpty { recordsJson += ";" }
                                recordsJson += "\(tnf):\(typeHex):\(payloadHex)"
                            }
                        }
                        on_nfc_tag_discovered_raw(self.tagCtx, "", "4", recordsJson.isEmpty ? nil : recordsJson)
                    }
                }
            }
        }
    }

    func readerSessionDidBecomeActive(_ session: NFCNDEFReaderSession) {}

    func readerSession(_ session: NFCNDEFReaderSession, didInvalidateWithError error: Error) {
        reportSessionError(error.localizedDescription)
        activeSessions.removeValue(forKey: tagCtx)
    }

    private func reportSessionError(_ message: String) {
        errorCallback?.call(message)
        errorCallback = nil
    }

    private func parseRecords(_ json: String) -> [NFCNDEFPayload] {
        return json.split(separator: ";").compactMap { recStr in
            let parts = recStr.split(separator: ":", maxSplits: 2)
            guard parts.count == 3,
                  let tnfVal = UInt8(parts[0]),
                  let tnf = NFCTypeNameFormat(rawValue: tnfVal) else { return nil }
            let typeData = hexDecode(String(parts[1]))
            let payloadData = hexDecode(String(parts[2]))
            return NFCNDEFPayload(format: tnf, type: typeData, identifier: Data(), payload: payloadData)
        }
    }

    private func hexDecode(_ hex: String) -> Data {
        var data = Data()
        var index = hex.startIndex
        while index < hex.endIndex {
            let nextIndex = hex.index(index, offsetBy: 2, limitedBy: hex.endIndex) ?? hex.endIndex
            if let byte = UInt8(hex[index..<nextIndex], radix: 16) {
                data.append(byte)
            }
            index = nextIndex
        }
        return data
    }
}

#elseif os(macOS)

func nfc_is_available_bridge() -> Bool {
    return false
}

func nfc_start_session(
    message: RustStr,
    tag_ctx: UInt64,
    error_callback: @escaping (String) -> Void
) {
    error_callback("NFC not available on macOS")
}

func nfc_stop_session(tag_ctx: UInt64) {}

func nfc_write_message(
    tag_ctx: UInt64,
    records_json: RustStr,
    callback: @escaping (String) -> Void
) {
    callback("NFC not available on macOS")
}

#endif
