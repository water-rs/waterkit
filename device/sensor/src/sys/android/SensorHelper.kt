package waterkit.sensor

import android.content.Context
import android.hardware.Sensor
import android.hardware.SensorEvent
import android.hardware.SensorEventListener
import android.hardware.SensorManager
import android.os.Handler
import android.os.Looper
import waterkit.build.NativeCallback

/**
 * Helper class for accessing sensors on Android.
 * Compiled to DEX and embedded in the Rust library.
 */
object SensorHelper {

    // Sensor type constants matching Android SDK
    const val TYPE_ACCELEROMETER = 1
    const val TYPE_GYROSCOPE = 4
    const val TYPE_MAGNETIC_FIELD = 2
    const val TYPE_PRESSURE = 6
    const val TYPE_LIGHT = 5

    private val mainHandler = Handler(Looper.getMainLooper())

    /**
     * Check if a sensor type is available on this device.
     */
    @JvmStatic
    fun isSensorAvailable(context: Context, sensorType: Int): Boolean {
        val manager = context.getSystemService(Context.SENSOR_SERVICE) as? SensorManager
            ?: return false
        return manager.getDefaultSensor(sensorType) != null
    }

    /**
     * Take a one-shot reading of [sensorType]: the listener completes
     * [callback] with `[x, y, z, timestamp]` (epoch milliseconds) on its first
     * event and unregisters itself. [callback] fails when the sensor is absent
     * or registration is refused.
     */
    @JvmStatic
    fun readSensor(context: Context, sensorType: Int, callback: NativeCallback) {
        readFirstEvent(context, sensorType, 3, callback) { event ->
            doubleArrayOf(
                event.values[0].toDouble(),
                event.values[1].toDouble(),
                event.values[2].toDouble(),
                event.timestamp.toDouble() / 1_000_000.0 // ns to ms
            )
        }
    }

    /**
     * Take a one-shot pressure reading: completes [callback] with
     * `[hectopascals, timestamp]` (epoch milliseconds).
     */
    @JvmStatic
    fun readPressure(context: Context, callback: NativeCallback) {
        readFirstEvent(context, Sensor.TYPE_PRESSURE, 1, callback) { event ->
            doubleArrayOf(
                event.values[0].toDouble(),
                event.timestamp.toDouble() / 1_000_000.0
            )
        }
    }

    /**
     * Take a one-shot ambient light reading: completes [callback] with
     * `[lux, timestamp]` (epoch milliseconds).
     */
    @JvmStatic
    fun readLight(context: Context, callback: NativeCallback) {
        readFirstEvent(context, Sensor.TYPE_LIGHT, 1, callback) { event ->
            doubleArrayOf(
                event.values[0].toDouble(),
                event.timestamp.toDouble() / 1_000_000.0
            )
        }
    }

    /**
     * Registers a listener for [sensorType] on the main looper; the first
     * event carrying at least [minimumValues] values is shaped by [transform]
     * and completes [callback], then the listener unregisters itself.
     */
    private fun readFirstEvent(
        context: Context,
        sensorType: Int,
        minimumValues: Int,
        callback: NativeCallback,
        transform: (SensorEvent) -> DoubleArray,
    ) {
        val manager = context.getSystemService(Context.SENSOR_SERVICE) as? SensorManager
            ?: return callback.fail("sensor service is unavailable")
        val sensor = manager.getDefaultSensor(sensorType)
            ?: return callback.fail("sensor type $sensorType is not available")

        val listener = object : SensorEventListener {
            override fun onSensorChanged(event: SensorEvent) {
                manager.unregisterListener(this)
                if (event.values.size >= minimumValues) {
                    callback.complete(transform(event))
                } else {
                    callback.fail(
                        "sensor type $sensorType reported too few values: ${event.values.size}"
                    )
                }
            }

            override fun onAccuracyChanged(sensor: Sensor, accuracy: Int) {}
        }

        if (!manager.registerListener(listener, sensor, SensorManager.SENSOR_DELAY_GAME, mainHandler)) {
            callback.fail("sensor type $sensorType refused registration")
        }
    }
}
