package waterkit.sensor

import android.content.Context
import android.hardware.Sensor
import android.hardware.SensorEvent
import android.hardware.SensorEventListener
import android.hardware.SensorManager
import android.os.Handler
import android.os.Looper
import waterkit.build.NativeCallback
import waterkit.build.NativeChannel

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

    internal val mainHandler = Handler(Looper.getMainLooper())

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
    /**
     * Register a [SensorWatch] for [sensorType] at [samplingPeriodUs]
     * microseconds and return it. Throws `IllegalStateException` when the
     * sensor is absent or registration is refused.
     */
    @JvmStatic
    fun watchSensor(
        context: Context,
        sensorType: Int,
        samplingPeriodUs: Int,
        channel: NativeChannel,
    ): SensorWatch {
        val manager = context.getSystemService(Context.SENSOR_SERVICE) as? SensorManager
            ?: throw IllegalStateException("sensor service is unavailable")
        val sensor = manager.getDefaultSensor(sensorType)
            ?: throw IllegalStateException("sensor type $sensorType is not available")
        val watch = SensorWatch(manager, channel)
        watch.start(sensor, samplingPeriodUs)
        return watch
    }

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

/**
 * One sensor watch: a `SensorEventListener` registered at the requested
 * sampling period, streaming `[values..., epoch_ms]` payloads into its
 * [NativeChannel]. The Rust stream handle owns it as a global reference;
 * [stop] unregisters the listener and ends the stream.
 */
class SensorWatch internal constructor(
    private val manager: SensorManager,
    private val channel: NativeChannel,
) {
    private val listener = object : SensorEventListener {
        override fun onSensorChanged(event: SensorEvent) {
            channel.send(
                DoubleArray(event.values.size + 1) { i ->
                    if (i < event.values.size) event.values[i].toDouble()
                    else event.timestamp.toDouble() / 1_000_000.0
                }
            )
        }

        override fun onAccuracyChanged(sensor: Sensor, accuracy: Int) {}
    }

    /** Registers the listener at [samplingPeriodUs] on the main looper. */
    internal fun start(sensor: Sensor, samplingPeriodUs: Int) {
        check(manager.registerListener(listener, sensor, samplingPeriodUs, SensorHelper.mainHandler)) {
            "sensor type ${sensor.type} refused registration"
        }
    }

    /** Unregisters the listener and ends the stream. Called again is a no-op. */
    fun stop() {
        manager.unregisterListener(listener)
        channel.close()
    }
}
