import Foundation
import CoreBluetooth

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

func bluetooth_subscribe(device_id: RustStr, service_uuid: RustStr, char_uuid: RustStr, cb_id: UInt64) {
    let idStr = device_id.toString()
    let svcUuid = CBUUID(string: service_uuid.toString())
    let chrUuid = CBUUID(string: char_uuid.toString())
    guard let peripheral = peripherals[idStr] else { return }
    guard let service = peripheral.services?.first(where: { $0.uuid == svcUuid }),
          let characteristic = service.characteristics?.first(where: { $0.uuid == chrUuid }) else { return }
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
        on_scan_result(cbId, deviceId, peripheral.name, Int16(truncating: RSSI), svcUuids)
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
            if let data = characteristic.value {
                on_notify_value(deviceId, charUuid, Array(data))
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
