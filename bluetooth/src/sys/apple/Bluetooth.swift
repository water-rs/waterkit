import Foundation
import CoreBluetooth
#if os(macOS)
import IOBluetooth
#endif

private var centralManager: CBCentralManager?
private var centralDelegate: CentralManagerDelegate?
private var peripherals: [String: CBPeripheral] = [:]
private var peripheralDelegates: [String: PeripheralDelegate] = [:]

func bluetooth_adapter_state(cb_id: UInt64) {
    if centralDelegate == nil {
        centralDelegate = CentralManagerDelegate()
        centralManager = CBCentralManager(delegate: centralDelegate!, queue: .main)
    }
    centralDelegate!.pendingStateCallbacks.append(cb_id)
    if centralManager!.state != .unknown {
        centralDelegate!.flushStateCallbacks(centralManager!.state)
    }
}

func bluetooth_start_scan(cb_id: UInt64, service_uuids: RustString) {
    guard let cm = centralManager else { return }
    centralDelegate?.scanCallbackId = cb_id
    let uuidStr = service_uuids.toString()
    var uuids: [CBUUID]? = nil
    if !uuidStr.isEmpty {
        uuids = uuidStr.split(separator: ",").map { CBUUID(string: String($0)) }
    }
    cm.scanForPeripherals(withServices: uuids, options: [
        CBCentralManagerScanOptionAllowDuplicatesKey: false
    ])
}

func bluetooth_stop_scan(cb_id: UInt64) {
    centralManager?.stopScan()
    centralDelegate?.scanCallbackId = nil
}

func bluetooth_connect(device_id: RustStr, cb_id: UInt64) {
    let idStr = device_id.toString()
    guard let cm = centralManager, let peripheral = peripherals[idStr] else {
        on_connect_result(cb_id, "Device not found")
        return
    }
    centralDelegate?.connectCallbacks[idStr] = cb_id
    cm.connect(peripheral, options: nil)
}

func bluetooth_disconnect(device_id: RustStr) {
    let idStr = device_id.toString()
    guard let cm = centralManager, let peripheral = peripherals[idStr] else { return }
    cm.cancelPeripheralConnection(peripheral)
}

func bluetooth_discover_services(device_id: RustStr, cb_id: UInt64) {
    let idStr = device_id.toString()
    guard let peripheral = peripherals[idStr] else {
        on_discover_services_result(cb_id, nil, "Device not found")
        return
    }
    let delegate = peripheralDelegates[idStr] ?? PeripheralDelegate(deviceId: idStr)
    peripheralDelegates[idStr] = delegate
    peripheral.delegate = delegate
    delegate.discoverServicesCallback = cb_id
    peripheral.discoverServices(nil)
}

func bluetooth_read_characteristic(device_id: RustStr, service_uuid: RustStr, char_uuid: RustStr, cb_id: UInt64) {
    let idStr = device_id.toString()
    let svcUuid = CBUUID(string: service_uuid.toString())
    let chrUuid = CBUUID(string: char_uuid.toString())
    guard let peripheral = peripherals[idStr] else {
        on_read_result(cb_id, nil, "Device not found")
        return
    }
    guard let service = peripheral.services?.first(where: { $0.uuid == svcUuid }),
          let characteristic = service.characteristics?.first(where: { $0.uuid == chrUuid }) else {
        on_read_result(cb_id, nil, "Characteristic not found")
        return
    }
    let delegate = peripheralDelegates[idStr]
    delegate?.readCallbacks[chrUuid.uuidString] = cb_id
    peripheral.readValue(for: characteristic)
}

func bluetooth_write_characteristic(device_id: RustStr, service_uuid: RustStr, char_uuid: RustStr, data: RustSlice<UInt8>, cb_id: UInt64) {
    let idStr = device_id.toString()
    let svcUuid = CBUUID(string: service_uuid.toString())
    let chrUuid = CBUUID(string: char_uuid.toString())
    guard let peripheral = peripherals[idStr] else {
        on_write_result(cb_id, "Device not found")
        return
    }
    guard let service = peripheral.services?.first(where: { $0.uuid == svcUuid }),
          let characteristic = service.characteristics?.first(where: { $0.uuid == chrUuid }) else {
        on_write_result(cb_id, "Characteristic not found")
        return
    }
    let delegate = peripheralDelegates[idStr]
    delegate?.writeCallbacks[chrUuid.uuidString] = cb_id
    let bytes = Data(bytes: data.start(), count: data.len())
    peripheral.writeValue(bytes, for: characteristic, type: .withResponse)
}

func bluetooth_subscribe(device_id: RustStr, service_uuid: RustStr, char_uuid: RustStr, notify_ctx: UInt64) {
    let idStr = device_id.toString()
    let svcUuid = CBUUID(string: service_uuid.toString())
    let chrUuid = CBUUID(string: char_uuid.toString())
    guard let peripheral = peripherals[idStr] else { return }
    guard let service = peripheral.services?.first(where: { $0.uuid == svcUuid }),
          let characteristic = service.characteristics?.first(where: { $0.uuid == chrUuid }) else { return }
    let delegate = peripheralDelegates[idStr] ?? PeripheralDelegate(deviceId: idStr)
    peripheralDelegates[idStr] = delegate
    peripheral.delegate = delegate
    delegate.notifyCallbacks[chrUuid.uuidString] = notify_ctx
    peripheral.setNotifyValue(true, for: characteristic)
}

class CentralManagerDelegate: NSObject, CBCentralManagerDelegate {
    var pendingStateCallbacks: [UInt64] = []
    var scanCallbackId: UInt64? = nil
    var connectCallbacks: [String: UInt64] = [:]

    func centralManagerDidUpdateState(_ central: CBCentralManager) {
        flushStateCallbacks(central.state)
    }

    func flushStateCallbacks(_ state: CBManagerState) {
        let stateStr: String
        switch state {
        case .poweredOn: stateStr = "poweredOn"
        case .poweredOff: stateStr = "poweredOff"
        case .unauthorized: stateStr = "unauthorized"
        case .unsupported: stateStr = "unsupported"
        case .resetting: stateStr = "resetting"
        default: stateStr = "unknown"
        }
        for cbId in pendingStateCallbacks {
            on_adapter_state_result(cbId, stateStr)
        }
        pendingStateCallbacks.removeAll()
    }

    func centralManager(_ central: CBCentralManager, didDiscover peripheral: CBPeripheral,
                        advertisementData: [String: Any], rssi RSSI: NSNumber) {
        let deviceId = peripheral.identifier.uuidString
        peripherals[deviceId] = peripheral
        guard let cbId = scanCallbackId else { return }
        var svcUuids = ""
        if let uuids = advertisementData[CBAdvertisementDataServiceUUIDsKey] as? [CBUUID] {
            svcUuids = uuids.map { $0.uuidString }.joined(separator: ",")
        }
        on_scan_result_raw(cbId, deviceId, peripheral.name, Int16(truncating: RSSI), svcUuids)
    }

    func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) {
        let deviceId = peripheral.identifier.uuidString
        if let cbId = connectCallbacks.removeValue(forKey: deviceId) {
            on_connect_result(cbId, nil)
        }
    }

    func centralManager(_ central: CBCentralManager, didFailToConnect peripheral: CBPeripheral, error: Error?) {
        let deviceId = peripheral.identifier.uuidString
        if let cbId = connectCallbacks.removeValue(forKey: deviceId) {
            on_connect_result(cbId, error?.localizedDescription ?? "Unknown error")
        }
    }
}

class PeripheralDelegate: NSObject, CBPeripheralDelegate {
    let deviceId: String
    var discoverServicesCallback: UInt64? = nil
    var readCallbacks: [String: UInt64] = [:]
    var writeCallbacks: [String: UInt64] = [:]
    var notifyCallbacks: [String: UInt64] = [:]

    init(deviceId: String) {
        self.deviceId = deviceId
        super.init()
    }

    func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
        guard let cbId = discoverServicesCallback else { return }
        discoverServicesCallback = nil
        if let error = error {
            on_discover_services_result(cbId, nil, error.localizedDescription)
            return
        }
        guard let services = peripheral.services else {
            on_discover_services_result(cbId, "", nil)
            return
        }
        for service in services {
            peripheral.discoverCharacteristics(nil, for: service)
        }
        // Wait for characteristics - store pending count
        pendingServiceCount = services.count
        discoveredServicesJson = ""
        self.discoverServicesCbId = cbId
    }

    private var pendingServiceCount = 0
    private var discoveredServicesJson = ""
    private var discoverServicesCbId: UInt64? = nil

    func peripheral(_ peripheral: CBPeripheral, didDiscoverCharacteristicsFor service: CBService, error: Error?) {
        pendingServiceCount -= 1
        let isPrimary = service.isPrimary ? "1" : "0"
        var chars = ""
        if let characteristics = service.characteristics {
            chars = characteristics.map { c in
                let r = c.properties.contains(.read) ? "1" : "0"
                let w = c.properties.contains(.write) ? "1" : "0"
                let wn = c.properties.contains(.writeWithoutResponse) ? "1" : "0"
                let n = c.properties.contains(.notify) ? "1" : "0"
                let i = c.properties.contains(.indicate) ? "1" : "0"
                return "\(c.uuid.uuidString):\(r):\(w):\(wn):\(n):\(i)"
            }.joined(separator: ",")
        }
        discoveredServicesJson += "\(service.uuid.uuidString):\(isPrimary):\(chars);"
        if pendingServiceCount <= 0, let cbId = discoverServicesCbId {
            discoverServicesCbId = nil
            on_discover_services_result(cbId, discoveredServicesJson, nil)
        }
    }

    func peripheral(_ peripheral: CBPeripheral, didUpdateValueFor characteristic: CBCharacteristic, error: Error?) {
        let charUuid = characteristic.uuid.uuidString
        if let cbId = readCallbacks.removeValue(forKey: charUuid) {
            if let error = error {
                on_read_result(cbId, nil, error.localizedDescription)
            } else {
                let data = characteristic.value.map { Array($0) } ?? []
                on_read_result(cbId, data, nil)
            }
        } else {
            // Notification
            if let data = characteristic.value,
               let notifyCtx = notifyCallbacks[charUuid] {
                let rustData = RustVec<UInt8>()
                for byte in data {
                    rustData.push(value: byte)
                }
                on_notify_value_raw(notifyCtx, rustData)
            }
        }
    }

    func peripheral(_ peripheral: CBPeripheral, didWriteValueFor characteristic: CBCharacteristic, error: Error?) {
        let charUuid = characteristic.uuid.uuidString
        if let cbId = writeCallbacks.removeValue(forKey: charUuid) {
            if let error = error {
                on_write_result(cbId, error.localizedDescription)
            } else {
                on_write_result(cbId, nil)
            }
        }
    }
}

#if os(macOS)
private final class ClassicBluetoothManager: NSObject, IOBluetoothDeviceInquiryDelegate {
    static let shared = ClassicBluetoothManager()

    private var discoveryInquiry: IOBluetoothDeviceInquiry?
    private var discoveryCtx: UInt64?
    private var streams: [UInt64: ClassicSppStream] = [:]
    private var pendingConnectors: [UInt64: ClassicSppConnector] = [:]

    func startDiscovery(scanCtx: UInt64) -> String {
        stopDiscoveryInternal()

        guard let inquiry = IOBluetoothDeviceInquiry(delegate: self) else {
            return "Failed to create classic Bluetooth inquiry"
        }
        inquiry.searchType = IOBluetoothDeviceSearchTypes(kIOBluetoothDeviceSearchClassic)
        inquiry.updateNewDeviceNames = false
        let status = inquiry.start()
        guard status == kIOReturnSuccess else {
            return "Failed to start classic Bluetooth inquiry: \(status)"
        }

        discoveryInquiry = inquiry
        discoveryCtx = scanCtx
        return ""
    }

    func stopDiscovery(scanCtx: UInt64) {
        guard let discoveryCtx, discoveryCtx == scanCtx else {
            return
        }
        stopDiscoveryInternal()
    }

    private func stopDiscoveryInternal() {
        if let inquiry = discoveryInquiry {
            _ = inquiry.stop()
        }
        discoveryInquiry = nil
        discoveryCtx = nil
    }

    func pairedDevicesPayload() -> String {
        let pairedAny = IOBluetoothDevice.pairedDevices()
        guard let paired = pairedAny as? [IOBluetoothDevice] else {
            return ""
        }

        var lines: [String] = []
        lines.reserveCapacity(paired.count)
        for device in paired {
            let address = sanitizeField(device.addressString)
            guard !address.isEmpty else {
                continue
            }
            let name = sanitizeField(device.name ?? "")
            let cod = UInt32(device.classOfDevice)
            let connected = device.isConnected() ? "1" : "0"
            lines.append("\(address)\t\(name)\t\(cod)\t\(connected)")
        }
        return lines.joined(separator: "\n")
    }

    func connectSpp(deviceId: String, uuid: String, streamCtx: UInt64, connectCtx: UInt64) {
        guard let device = IOBluetoothDevice(addressString: deviceId) else {
            on_classic_connect_result_raw(connectCtx, "Classic Bluetooth device not found: \(deviceId)")
            return
        }
        guard let sdpUuid = parseSdpUuid(uuid) else {
            on_classic_connect_result_raw(connectCtx, "Invalid SPP UUID: \(uuid)")
            return
        }

        let connector = ClassicSppConnector(
            manager: self,
            device: device,
            sdpUuid: sdpUuid,
            streamCtx: streamCtx,
            connectCtx: connectCtx
        )
        pendingConnectors[connectCtx] = connector
        let status = device.performSDPQuery(connector, uuids: [sdpUuid])
        if status != kIOReturnSuccess {
            pendingConnectors.removeValue(forKey: connectCtx)
            on_classic_connect_result_raw(connectCtx, "SPP SDP query start failed: \(status)")
        }
    }

    func handleConnectorSuccess(connectCtx: UInt64, stream: ClassicSppStream) {
        pendingConnectors.removeValue(forKey: connectCtx)
        streams[stream.streamCtx] = stream
        on_classic_connect_result_raw(connectCtx, "")
    }

    func handleConnectorFailure(connectCtx: UInt64, error: String) {
        pendingConnectors.removeValue(forKey: connectCtx)
        on_classic_connect_result_raw(connectCtx, error)
    }

    func readSpp(streamCtx: UInt64, maxBytes: UInt64, readCtx: UInt64) {
        guard let stream = streams[streamCtx] else {
            on_classic_spp_read_result_raw(readCtx, RustVec<UInt8>(), "SPP stream not found")
            return
        }
        stream.enqueueRead(maxBytes: maxBytes, readCtx: readCtx)
    }

    func writeSpp(streamCtx: UInt64, bytes: UnsafeBufferPointer<UInt8>, writeCtx: UInt64) {
        guard let stream = streams[streamCtx] else {
            on_classic_spp_write_result_raw(writeCtx, 0, "SPP stream not found")
            return
        }
        stream.enqueueWrite(bytes: bytes, writeCtx: writeCtx)
    }

    func closeSpp(streamCtx: UInt64, closeCtx: UInt64) {
        guard let stream = streams.removeValue(forKey: streamCtx) else {
            on_classic_spp_close_result_raw(closeCtx, "")
            return
        }
        stream.close(closeCtx: closeCtx)
    }

    func streamDidClose(_ streamCtx: UInt64) {
        streams.removeValue(forKey: streamCtx)
    }

    func deviceInquiryDeviceFound(_ sender: IOBluetoothDeviceInquiry!, device: IOBluetoothDevice!) {
        guard let device, let discoveryCtx else {
            return
        }
        let address = sanitizeField(device.addressString)
        guard !address.isEmpty else {
            return
        }
        let name = device.name
        on_classic_scan_result_raw(
            discoveryCtx,
            address,
            name,
            UInt32(device.classOfDevice),
            device.isPaired(),
            device.isConnected()
        )
    }

    func deviceInquiryComplete(_ sender: IOBluetoothDeviceInquiry!, error: IOReturn, aborted: Bool) {
        guard let discoveryInquiry, sender == discoveryInquiry else {
            return
        }
        self.discoveryInquiry = nil
        discoveryCtx = nil
        if !aborted && error != kIOReturnSuccess {
            assertionFailure("waterkit-bluetooth: classic inquiry completed with error \(error)")
        }
    }
}

private final class ClassicSppConnector: NSObject, IOBluetoothDeviceAsyncCallbacks, IOBluetoothRFCOMMChannelDelegate {
    private unowned let manager: ClassicBluetoothManager
    private let device: IOBluetoothDevice
    private let sdpUuid: IOBluetoothSDPUUID
    private let streamCtx: UInt64
    private let connectCtx: UInt64

    init(
        manager: ClassicBluetoothManager,
        device: IOBluetoothDevice,
        sdpUuid: IOBluetoothSDPUUID,
        streamCtx: UInt64,
        connectCtx: UInt64
    ) {
        self.manager = manager
        self.device = device
        self.sdpUuid = sdpUuid
        self.streamCtx = streamCtx
        self.connectCtx = connectCtx
        super.init()
    }

    func remoteNameRequestComplete(_ device: IOBluetoothDevice!, status: IOReturn) {}

    func connectionComplete(_ device: IOBluetoothDevice!, status: IOReturn) {}

    func sdpQueryComplete(_ device: IOBluetoothDevice!, status: IOReturn) {
        guard status == kIOReturnSuccess else {
            manager.handleConnectorFailure(
                connectCtx: connectCtx,
                error: "SPP SDP query failed: \(status)"
            )
            return
        }
        guard let record = self.device.getServiceRecord(for: sdpUuid) else {
            manager.handleConnectorFailure(
                connectCtx: connectCtx,
                error: "SPP service record not found for UUID"
            )
            return
        }
        var channelId: BluetoothRFCOMMChannelID = 0
        let channelStatus = record.getRFCOMMChannelID(&channelId)
        guard channelStatus == kIOReturnSuccess else {
            manager.handleConnectorFailure(
                connectCtx: connectCtx,
                error: "SPP RFCOMM channel discovery failed: \(channelStatus)"
            )
            return
        }
        var channel: IOBluetoothRFCOMMChannel?
        let openStatus = self.device.openRFCOMMChannelAsync(
            &channel,
            withChannelID: channelId,
            delegate: self
        )
        guard openStatus == kIOReturnSuccess else {
            manager.handleConnectorFailure(
                connectCtx: connectCtx,
                error: "SPP channel open start failed: \(openStatus)"
            )
            return
        }
    }

    func rfcommChannelOpenComplete(_ rfcommChannel: IOBluetoothRFCOMMChannel!, status error: IOReturn) {
        guard error == kIOReturnSuccess, let channel = rfcommChannel else {
            manager.handleConnectorFailure(
                connectCtx: connectCtx,
                error: "SPP channel open failed: \(error)"
            )
            return
        }
        let stream = ClassicSppStream(manager: manager, streamCtx: streamCtx, channel: channel)
        manager.handleConnectorSuccess(connectCtx: connectCtx, stream: stream)
    }
}

private final class ClassicSppStream: NSObject, IOBluetoothRFCOMMChannelDelegate {
    private unowned let manager: ClassicBluetoothManager
    let streamCtx: UInt64
    private let channel: IOBluetoothRFCOMMChannel
    private var readBuffer = Data()
    private var pendingReads: [(maxBytes: UInt64, readCtx: UInt64)] = []
    private var pendingWrites: [UInt64: NSMutableData] = [:]
    private var isClosed = false

    init(manager: ClassicBluetoothManager, streamCtx: UInt64, channel: IOBluetoothRFCOMMChannel) {
        self.manager = manager
        self.streamCtx = streamCtx
        self.channel = channel
        super.init()
        let status = channel.setDelegate(self)
        assert(status == kIOReturnSuccess, "waterkit-bluetooth: failed to set RFCOMM delegate: \(status)")
    }

    func enqueueRead(maxBytes: UInt64, readCtx: UInt64) {
        if isClosed {
            on_classic_spp_read_result_raw(readCtx, RustVec<UInt8>(), "SPP stream closed")
            return
        }
        if !readBuffer.isEmpty {
            fulfillRead(maxBytes: maxBytes, readCtx: readCtx)
            return
        }
        pendingReads.append((maxBytes, readCtx))
    }

    func enqueueWrite(bytes: UnsafeBufferPointer<UInt8>, writeCtx: UInt64) {
        if isClosed {
            on_classic_spp_write_result_raw(writeCtx, 0, "SPP stream closed")
            return
        }
        if bytes.count > Int(UInt16.max) {
            on_classic_spp_write_result_raw(
                writeCtx,
                0,
                "SPP write payload exceeds UInt16.max (\(bytes.count))"
            )
            return
        }
        let payload = NSMutableData(bytes: bytes.baseAddress, length: bytes.count)
        pendingWrites[writeCtx] = payload
        let refcon = UnsafeMutableRawPointer(bitPattern: UInt(writeCtx))
        let status = channel.writeAsync(payload.mutableBytes, length: UInt16(payload.length), refcon: refcon)
        if status != kIOReturnSuccess {
            pendingWrites.removeValue(forKey: writeCtx)
            on_classic_spp_write_result_raw(writeCtx, 0, "SPP write start failed: \(status)")
        }
    }

    func close(closeCtx: UInt64) {
        if isClosed {
            on_classic_spp_close_result_raw(closeCtx, "")
            return
        }
        isClosed = true
        let status = channel.close()
        for pending in pendingReads {
            on_classic_spp_read_result_raw(pending.readCtx, RustVec<UInt8>(), "SPP stream closed")
        }
        pendingReads.removeAll()
        for (writeCtx, _) in pendingWrites {
            on_classic_spp_write_result_raw(writeCtx, 0, "SPP stream closed")
        }
        pendingWrites.removeAll()
        if status == kIOReturnSuccess {
            on_classic_spp_close_result_raw(closeCtx, "")
        } else {
            on_classic_spp_close_result_raw(closeCtx, "SPP close failed: \(status)")
        }
    }

    func rfcommChannelData(_ rfcommChannel: IOBluetoothRFCOMMChannel!, data dataPointer: UnsafeMutableRawPointer!, length dataLength: Int) {
        guard !isClosed else {
            return
        }
        guard dataLength > 0, let dataPointer else {
            return
        }
        readBuffer.append(dataPointer.assumingMemoryBound(to: UInt8.self), count: dataLength)
        drainPendingReads()
    }

    func rfcommChannelClosed(_ rfcommChannel: IOBluetoothRFCOMMChannel!) {
        if !isClosed {
            close(closeCtx: 0)
        }
        manager.streamDidClose(streamCtx)
    }

    func rfcommChannelWriteComplete(
        _ rfcommChannel: IOBluetoothRFCOMMChannel!,
        refcon: UnsafeMutableRawPointer!,
        status error: IOReturn
    ) {
        guard let refcon else {
            return
        }
        let writeCtx = UInt64(UInt(bitPattern: refcon))
        let payload = pendingWrites.removeValue(forKey: writeCtx)
        if error == kIOReturnSuccess {
            on_classic_spp_write_result_raw(writeCtx, UInt64(payload?.length ?? 0), "")
        } else {
            on_classic_spp_write_result_raw(writeCtx, 0, "SPP write failed: \(error)")
        }
    }

    func rfcommChannelWriteComplete(
        _ rfcommChannel: IOBluetoothRFCOMMChannel!,
        refcon: UnsafeMutableRawPointer!,
        status error: IOReturn,
        bytesWritten length: Int
    ) {
        guard let refcon else {
            return
        }
        let writeCtx = UInt64(UInt(bitPattern: refcon))
        _ = pendingWrites.removeValue(forKey: writeCtx)
        if error == kIOReturnSuccess {
            on_classic_spp_write_result_raw(writeCtx, UInt64(length), "")
        } else {
            on_classic_spp_write_result_raw(writeCtx, 0, "SPP write failed: \(error)")
        }
    }

    private func drainPendingReads() {
        while !pendingReads.isEmpty && !readBuffer.isEmpty {
            let pending = pendingReads.removeFirst()
            fulfillRead(maxBytes: pending.maxBytes, readCtx: pending.readCtx)
        }
    }

    private func fulfillRead(maxBytes: UInt64, readCtx: UInt64) {
        let capped = min(maxBytes, UInt64(readBuffer.count))
        let count = Int(capped)
        let chunk = readBuffer.prefix(count)
        readBuffer.removeFirst(count)
        on_classic_spp_read_result_raw(readCtx, rustVec(from: chunk), "")
    }
}

private func rustVec<DataLike: DataProtocol>(from bytes: DataLike) -> RustVec<UInt8> {
    let vec = RustVec<UInt8>()
    for byte in bytes {
        vec.push(value: byte)
    }
    return vec
}

private func sanitizeField(_ field: String) -> String {
    field
        .replacingOccurrences(of: "\t", with: " ")
        .replacingOccurrences(of: "\n", with: " ")
        .trimmingCharacters(in: .whitespacesAndNewlines)
}

private func parseSdpUuid(_ uuid: String) -> IOBluetoothSDPUUID? {
    let hex = uuid
        .trimmingCharacters(in: .whitespacesAndNewlines)
        .replacingOccurrences(of: "-", with: "")
        .lowercased()
    guard hex.count == 4 || hex.count == 8 || hex.count == 32 else {
        return nil
    }

    var bytes: [UInt8] = []
    bytes.reserveCapacity(hex.count / 2)
    var index = hex.startIndex
    while index < hex.endIndex {
        let next = hex.index(index, offsetBy: 2)
        let part = hex[index..<next]
        guard let byte = UInt8(part, radix: 16) else {
            return nil
        }
        bytes.append(byte)
        index = next
    }
    return IOBluetoothSDPUUID(bytes: bytes, length: bytes.count)
}
#endif

func bluetooth_classic_is_available() -> Bool {
    #if os(macOS)
    return true
    #else
    return false
    #endif
}

func bluetooth_classic_start_discovery(scan_ctx: UInt64) -> RustString {
    #if os(macOS)
    let error = ClassicBluetoothManager.shared.startDiscovery(scanCtx: scan_ctx)
    return RustString(error)
    #else
    let _ = scan_ctx
    return RustString("Classic Bluetooth is not available on iOS")
    #endif
}

func bluetooth_classic_stop_discovery(scan_ctx: UInt64) {
    #if os(macOS)
    ClassicBluetoothManager.shared.stopDiscovery(scanCtx: scan_ctx)
    #else
    let _ = scan_ctx
    #endif
}

func bluetooth_classic_paired_devices(query_ctx: UInt64) {
    #if os(macOS)
    let payload = ClassicBluetoothManager.shared.pairedDevicesPayload()
    on_classic_paired_devices_result_raw(query_ctx, payload, "")
    #else
    on_classic_paired_devices_result_raw(
        query_ctx,
        "",
        "Classic Bluetooth is not available on iOS"
    )
    #endif
}

func bluetooth_classic_connect_spp(
    device_id: RustStr,
    uuid: RustStr,
    stream_ctx: UInt64,
    connect_ctx: UInt64
) {
    #if os(macOS)
    ClassicBluetoothManager.shared.connectSpp(
        deviceId: device_id.toString(),
        uuid: uuid.toString(),
        streamCtx: stream_ctx,
        connectCtx: connect_ctx
    )
    #else
    let _ = device_id
    let _ = uuid
    let _ = stream_ctx
    on_classic_connect_result_raw(connect_ctx, "Classic Bluetooth is not available on iOS")
    #endif
}

func bluetooth_classic_spp_read(stream_ctx: UInt64, max_bytes: UInt64, read_ctx: UInt64) {
    #if os(macOS)
    ClassicBluetoothManager.shared.readSpp(streamCtx: stream_ctx, maxBytes: max_bytes, readCtx: read_ctx)
    #else
    let _ = stream_ctx
    let _ = max_bytes
    on_classic_spp_read_result_raw(read_ctx, RustVec<UInt8>(), "Classic Bluetooth is not available on iOS")
    #endif
}

func bluetooth_classic_spp_write(stream_ctx: UInt64, data: RustSlice<UInt8>, write_ctx: UInt64) {
    #if os(macOS)
    let bytes = UnsafeBufferPointer(start: data.start(), count: data.len())
    ClassicBluetoothManager.shared.writeSpp(streamCtx: stream_ctx, bytes: bytes, writeCtx: write_ctx)
    #else
    let _ = stream_ctx
    let _ = data
    on_classic_spp_write_result_raw(write_ctx, 0, "Classic Bluetooth is not available on iOS")
    #endif
}

func bluetooth_classic_spp_close(stream_ctx: UInt64, close_ctx: UInt64) {
    #if os(macOS)
    ClassicBluetoothManager.shared.closeSpp(streamCtx: stream_ctx, closeCtx: close_ctx)
    #else
    let _ = stream_ctx
    on_classic_spp_close_result_raw(close_ctx, "")
    #endif
}
