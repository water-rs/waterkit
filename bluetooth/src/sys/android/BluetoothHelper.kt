package waterkit.bluetooth

import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCallback
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothGattService
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.bluetooth.le.BluetoothLeScanner
import android.bluetooth.le.ScanCallback
import android.bluetooth.le.ScanFilter
import android.bluetooth.le.ScanResult
import android.bluetooth.le.ScanSettings
import android.content.Context
import android.os.ParcelUuid
import java.util.UUID

object BluetoothHelper {
    @JvmStatic
    fun getAdapterState(context: Context): Int {
        val manager = context.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager
            ?: return 2 // Unavailable
        val adapter = manager.adapter ?: return 2
        return if (adapter.isEnabled) 1 else 0 // PoweredOn : PoweredOff
    }

    @JvmStatic
    fun startBleScan(
        context: Context,
        serviceUuids: Array<String>?,
        callback: BleScanCallback
    ): BluetoothLeScanner? {
        val manager = context.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager
            ?: return null
        val scanner = manager.adapter?.bluetoothLeScanner ?: return null
        val filters = serviceUuids?.map { uuid ->
            ScanFilter.Builder()
                .setServiceUuid(ParcelUuid.fromString(uuid))
                .build()
        }
        val settings = ScanSettings.Builder()
            .setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY)
            .build()
        scanner.startScan(filters, settings, callback)
        return scanner
    }

    @JvmStatic
    fun stopBleScan(scanner: BluetoothLeScanner?, callback: ScanCallback?) {
        if (callback != null) {
            scanner?.stopScan(callback)
        }
    }

    @JvmStatic
    fun connectGatt(
        context: Context,
        deviceAddress: String,
        callback: BluetoothGattCallback
    ): BluetoothGatt? {
        val manager = context.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager
            ?: return null
        val device = manager.adapter?.getRemoteDevice(deviceAddress) ?: return null
        return device.connectGatt(context, false, callback)
    }

    @JvmStatic
    fun getPairedDevices(context: Context): Array<BluetoothDevice> {
        val manager = context.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager
            ?: return emptyArray()
        return manager.adapter?.bondedDevices?.toTypedArray() ?: emptyArray()
    }
}

abstract class BleScanCallback : ScanCallback() {
    abstract fun onResult(deviceAddress: String, deviceName: String?, rssi: Int, serviceUuids: Array<String>)

    override fun onScanResult(callbackType: Int, result: ScanResult) {
        val uuids = result.scanRecord?.serviceUuids?.map { it.uuid.toString() }?.toTypedArray()
            ?: emptyArray()
        onResult(
            result.device.address,
            result.device.name,
            result.rssi,
            uuids
        )
    }
}
