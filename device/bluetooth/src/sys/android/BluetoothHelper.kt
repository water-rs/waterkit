package waterkit.bluetooth

import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCallback
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothGattService
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.bluetooth.BluetoothSocket
import android.bluetooth.le.BluetoothLeScanner
import android.bluetooth.le.ScanCallback
import android.bluetooth.le.ScanFilter
import android.bluetooth.le.ScanResult
import android.bluetooth.le.ScanSettings
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.ParcelUuid
import java.util.UUID

object BluetoothHelper {
    private var classicDiscoveryReceiver: BroadcastReceiver? = null

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

    @JvmStatic
    fun startClassicDiscovery(
        context: Context,
        callback: ClassicDiscoveryCallback
    ): Boolean {
        val manager = context.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager
            ?: return false
        val adapter = manager.adapter ?: return false
        if (!adapter.isEnabled) {
            return false
        }

        stopClassicDiscovery(context)

        val appContext = context.applicationContext
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(ctx: Context?, intent: Intent?) {
                if (intent?.action != BluetoothDevice.ACTION_FOUND) {
                    return
                }
                val device = intent.extras?.get(BluetoothDevice.EXTRA_DEVICE) as? BluetoothDevice
                    ?: return
                val majorClass = device.bluetoothClass?.majorDeviceClass ?: 0
                callback.onDeviceFound(
                    device.address,
                    device.name,
                    majorClass,
                    device.bondState == BluetoothDevice.BOND_BONDED
                )
            }
        }

        val filter = IntentFilter().apply {
            addAction(BluetoothDevice.ACTION_FOUND)
        }
        appContext.registerReceiver(receiver, filter)
        classicDiscoveryReceiver = receiver

        if (adapter.isDiscovering) {
            adapter.cancelDiscovery()
        }
        return adapter.startDiscovery()
    }

    @JvmStatic
    fun stopClassicDiscovery(context: Context) {
        val manager = context.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager
        val adapter = manager?.adapter
        if (adapter?.isDiscovering == true) {
            adapter.cancelDiscovery()
        }

        val appContext = context.applicationContext
        val receiver = classicDiscoveryReceiver
        if (receiver != null) {
            try {
                appContext.unregisterReceiver(receiver)
            } catch (_: IllegalArgumentException) {
                // Receiver can already be unregistered when host tears down.
            }
            classicDiscoveryReceiver = null
        }
    }

    @JvmStatic
    fun connectSpp(context: Context, deviceAddress: String, serviceUuid: String): BluetoothSocket? {
        val manager = context.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager
            ?: return null
        val adapter = manager.adapter ?: return null
        val device = adapter.getRemoteDevice(deviceAddress) ?: return null
        if (adapter.isDiscovering) {
            adapter.cancelDiscovery()
        }
        val socket = device.createRfcommSocketToServiceRecord(UUID.fromString(serviceUuid))
        socket.connect()
        return socket
    }

    @JvmStatic
    fun readSpp(socket: BluetoothSocket, maxBytes: Int): ByteArray? {
        val input = socket.inputStream
        val buffer = ByteArray(maxBytes)
        val read = input.read(buffer)
        if (read <= 0) {
            return null
        }
        return buffer.copyOf(read)
    }

    @JvmStatic
    fun writeSpp(socket: BluetoothSocket, data: ByteArray): Int {
        val output = socket.outputStream
        output.write(data)
        output.flush()
        return data.size
    }

    @JvmStatic
    fun closeSpp(socket: BluetoothSocket) {
        socket.close()
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
