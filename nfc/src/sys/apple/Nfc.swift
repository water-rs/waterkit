import Foundation
#if os(iOS)
import CoreNFC

private var activeSessions: [UInt64: NFCSession] = [:]

func nfc_is_available_bridge() -> Bool {
    return NFCNDEFReaderSession.readingAvailable
}

func nfc_start_session(message: RustStr, cb_id: UInt64) {
    let msg = message.toString()
    DispatchQueue.main.async {
        let session = NFCSession(callbackId: cb_id, message: msg)
        activeSessions[cb_id] = session
        session.start()
    }
}

func nfc_stop_session(cb_id: UInt64) {
    DispatchQueue.main.async {
        if let session = activeSessions.removeValue(forKey: cb_id) {
            session.stop()
        }
    }
}

func nfc_write_message(cb_id: UInt64, records_json: RustStr, write_cb_id: UInt64) {
    let json = records_json.toString()
    DispatchQueue.main.async {
        guard let session = activeSessions[cb_id] else {
            on_nfc_write_result(write_cb_id, "No active session")
            return
        }
        session.pendingWrite = (write_cb_id, json)
    }
}

class NFCSession: NSObject, NFCNDEFReaderSessionDelegate {
    let callbackId: UInt64
    var readerSession: NFCNDEFReaderSession?
    var pendingWrite: (UInt64, String)? = nil

    init(callbackId: UInt64, message: String) {
        self.callbackId = callbackId
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
            on_nfc_tag_discovered(callbackId, "", "4", recordsJson)
        }
    }

    func readerSession(_ session: NFCNDEFReaderSession, didDetect tags: [NFCNDEFTag]) {
        guard let tag = tags.first else { return }

        session.connect(to: tag) { error in
            if let error = error {
                on_nfc_session_error(self.callbackId, error.localizedDescription)
                return
            }

            tag.queryNDEFStatus { status, capacity, error in
                if let error = error {
                    on_nfc_session_error(self.callbackId, error.localizedDescription)
                    return
                }

                if let (writeCbId, recordsJson) = self.pendingWrite {
                    self.pendingWrite = nil
                    if status == .readWrite {
                        let records = self.parseRecords(recordsJson)
                        let ndefMessage = NFCNDEFMessage(records: records)
                        tag.writeNDEF(ndefMessage) { error in
                            if let error = error {
                                on_nfc_write_result(writeCbId, error.localizedDescription)
                            } else {
                                on_nfc_write_result(writeCbId, nil)
                            }
                        }
                    } else {
                        on_nfc_write_result(writeCbId, "Tag is read-only")
                    }
                } else {
                    tag.readNDEF { message, error in
                        if let error = error {
                            on_nfc_session_error(self.callbackId, error.localizedDescription)
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
                        on_nfc_tag_discovered(self.callbackId, "", "4", recordsJson.isEmpty ? nil : recordsJson)
                    }
                }
            }
        }
    }

    func readerSessionDidBecomeActive(_ session: NFCNDEFReaderSession) {}

    func readerSession(_ session: NFCNDEFReaderSession, didInvalidateWithError error: Error) {
        on_nfc_session_error(callbackId, error.localizedDescription)
        activeSessions.removeValue(forKey: callbackId)
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

func nfc_start_session(message: RustStr, cb_id: UInt64) {
    on_nfc_session_error(cb_id, "NFC not available on macOS")
}

func nfc_stop_session(cb_id: UInt64) {}

func nfc_write_message(cb_id: UInt64, records_json: RustStr, write_cb_id: UInt64) {
    on_nfc_write_result(write_cb_id, "NFC not available on macOS")
}

#endif
