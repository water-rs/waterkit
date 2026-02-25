package waterkit.bluetooth

class BleScanBridgeCallback : BleScanCallback() {
    @JvmField
    var waterkit_scan_state: Long = 0

    external fun onScanResultNative(
        deviceAddress: String,
        deviceName: String?,
        rssi: Int,
        serviceUuids: Array<String>
    )

    override fun onResult(
        deviceAddress: String,
        deviceName: String?,
        rssi: Int,
        serviceUuids: Array<String>
    ) {
        onScanResultNative(deviceAddress, deviceName, rssi, serviceUuids)
    }
}
