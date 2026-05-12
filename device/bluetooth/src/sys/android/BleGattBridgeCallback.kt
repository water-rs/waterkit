package waterkit.bluetooth

import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCallback
import android.bluetooth.BluetoothGattCharacteristic

class BleGattBridgeCallback : BluetoothGattCallback() {
    @JvmField
    var waterkit_gatt_state: Long = 0

    external fun onConnectionStateNative(deviceAddress: String, connected: Boolean, status: Int)
    external fun onServicesDiscoveredNative(deviceAddress: String, payload: String, status: Int)
    external fun onCharacteristicReadNative(
        deviceAddress: String,
        serviceUuid: String,
        characteristicUuid: String,
        value: ByteArray,
        status: Int
    )

    external fun onCharacteristicWriteNative(
        deviceAddress: String,
        serviceUuid: String,
        characteristicUuid: String,
        status: Int
    )

    external fun onCharacteristicChangedNative(
        deviceAddress: String,
        serviceUuid: String,
        characteristicUuid: String,
        value: ByteArray
    )

    override fun onConnectionStateChange(gatt: BluetoothGatt, status: Int, newState: Int) {
        onConnectionStateNative(
            gatt.device.address,
            newState == android.bluetooth.BluetoothProfile.STATE_CONNECTED,
            status
        )
    }

    override fun onServicesDiscovered(gatt: BluetoothGatt, status: Int) {
        onServicesDiscoveredNative(gatt.device.address, encodeServicesPayload(gatt), status)
    }

    override fun onCharacteristicRead(
        gatt: BluetoothGatt,
        characteristic: BluetoothGattCharacteristic,
        status: Int
    ) {
        onCharacteristicReadNative(
            gatt.device.address,
            characteristic.service.uuid.toString(),
            characteristic.uuid.toString(),
            characteristic.value ?: ByteArray(0),
            status
        )
    }

    override fun onCharacteristicRead(
        gatt: BluetoothGatt,
        characteristic: BluetoothGattCharacteristic,
        value: ByteArray,
        status: Int
    ) {
        onCharacteristicReadNative(
            gatt.device.address,
            characteristic.service.uuid.toString(),
            characteristic.uuid.toString(),
            value,
            status
        )
    }

    override fun onCharacteristicWrite(
        gatt: BluetoothGatt,
        characteristic: BluetoothGattCharacteristic,
        status: Int
    ) {
        onCharacteristicWriteNative(
            gatt.device.address,
            characteristic.service.uuid.toString(),
            characteristic.uuid.toString(),
            status
        )
    }

    override fun onCharacteristicChanged(gatt: BluetoothGatt, characteristic: BluetoothGattCharacteristic) {
        onCharacteristicChangedNative(
            gatt.device.address,
            characteristic.service.uuid.toString(),
            characteristic.uuid.toString(),
            characteristic.value ?: ByteArray(0)
        )
    }

    override fun onCharacteristicChanged(
        gatt: BluetoothGatt,
        characteristic: BluetoothGattCharacteristic,
        value: ByteArray
    ) {
        onCharacteristicChangedNative(
            gatt.device.address,
            characteristic.service.uuid.toString(),
            characteristic.uuid.toString(),
            value
        )
    }

    private fun encodeServicesPayload(gatt: BluetoothGatt): String {
        return gatt.services.joinToString(separator = ";", postfix = ";") { service ->
            val primary = if (service.type == android.bluetooth.BluetoothGattService.SERVICE_TYPE_PRIMARY) {
                "1"
            } else {
                "0"
            }
            val characteristics = service.characteristics.joinToString(separator = ",") { characteristic ->
                val props = characteristic.properties
                val read = if ((props and BluetoothGattCharacteristic.PROPERTY_READ) != 0) "1" else "0"
                val write = if ((props and BluetoothGattCharacteristic.PROPERTY_WRITE) != 0) "1" else "0"
                val writeNoResponse =
                    if ((props and BluetoothGattCharacteristic.PROPERTY_WRITE_NO_RESPONSE) != 0) {
                        "1"
                    } else {
                        "0"
                    }
                val notify = if ((props and BluetoothGattCharacteristic.PROPERTY_NOTIFY) != 0) "1" else "0"
                val indicate = if ((props and BluetoothGattCharacteristic.PROPERTY_INDICATE) != 0) "1" else "0"
                "${characteristic.uuid}:$read:$write:$writeNoResponse:$notify:$indicate"
            }
            "${service.uuid}:$primary:$characteristics"
        }
    }
}
