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

    @Synchronized
    override fun onResult(
        deviceAddress: String,
        deviceName: String?,
        rssi: Int,
        serviceUuids: Array<String>
    ) {
        if (waterkit_scan_state == 0L) {
            return
        }
        onScanResultNative(deviceAddress, deviceName, rssi, serviceUuids)
    }

    @Synchronized
    fun releaseNativeState() {
        waterkit_scan_state = 0
    }
}
