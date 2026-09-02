package waterkit.bluetooth

abstract class ClassicDiscoveryCallback {
    abstract fun onDeviceFound(
        deviceAddress: String,
        deviceName: String?,
        majorDeviceClass: Int,
        isPaired: Boolean
    )
}

class ClassicDiscoveryBridgeCallback : ClassicDiscoveryCallback() {
    @JvmField
    var waterkit_classic_discovery_state: Long = 0

    external fun onDeviceFoundNative(
        deviceAddress: String,
        deviceName: String?,
        majorDeviceClass: Int,
        isPaired: Boolean
    )

    @Synchronized
    override fun onDeviceFound(
        deviceAddress: String,
        deviceName: String?,
        majorDeviceClass: Int,
        isPaired: Boolean
    ) {
        if (waterkit_classic_discovery_state == 0L) {
            return
        }
        onDeviceFoundNative(deviceAddress, deviceName, majorDeviceClass, isPaired)
    }

    @Synchronized
    fun releaseNativeState() {
        waterkit_classic_discovery_state = 0
    }
}
