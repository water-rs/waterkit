package waterkit.sensor

import android.content.Context
import android.hardware.Sensor
import android.hardware.SensorEvent
import android.hardware.SensorEventListener
import android.hardware.SensorManager
import android.os.Handler
import android.os.HandlerThread
import android.os.SystemClock

/**
 * Helper class for accessing sensors on Android.
 * Compiled to DEX and embedded in the Rust library.
 */
object SensorHelper {

    // Sensor type constants matching Android SDK
    const val TYPE_ACCELEROMETER = 1
    const val TYPE_GYROSCOPE = 4
    const val TYPE_MAGNETOMETER = 2
    const val TYPE_PRESSURE = 6

    /** How long a read waits for the sensor's first sample. */
    private const val EVENT_TIMEOUT_MS = 1000L

    /**
     * Check if a sensor type is available.
     */
    @JvmStatic
    fun isSensorAvailable(context: Context, sensorType: Int): Boolean {
        val manager = context.getSystemService(Context.SENSOR_SERVICE) as? SensorManager
            ?: return false
        return manager.getDefaultSensor(sensorType) != null
    }

    /**
     * Read a 3-axis sensor (accelerometer, gyroscope, magnetometer).
     * Returns array: [success, x, y, z, timestamp]
     * On failure: [0.0]
     */
    @JvmStatic
    fun readSensor(context: Context, sensorType: Int): DoubleArray =
        awaitFirstEvent(context, sensorType, 3) { event ->
            doubleArrayOf(
                1.0, // success
                event.values[0].toDouble(),
                event.values[1].toDouble(),
                event.values[2].toDouble(),
                event.timestamp.toDouble() / 1_000_000.0 // ns to ms
            )
        }

    /**
     * Read pressure sensor (barometer).
     * Returns array: [success, pressure_hPa, timestamp]
     * On failure: [0.0]
     */
    @JvmStatic
    fun readPressure(context: Context): DoubleArray =
        readScalarSensor(context, Sensor.TYPE_PRESSURE)

    /**
     * Read ambient light sensor.
     * Returns array: [success, lux, timestamp]
     * On failure: [0.0]
     */
    @JvmStatic
    fun readLight(context: Context): DoubleArray =
        readScalarSensor(context, Sensor.TYPE_LIGHT)

    /**
     * Read a single-value sensor.
     * Returns array: [success, value, timestamp]
     * On failure: [0.0]
     */
    private fun readScalarSensor(context: Context, sensorType: Int): DoubleArray =
        awaitFirstEvent(context, sensorType, 1) { event ->
            doubleArrayOf(
                1.0, // success
                event.values[0].toDouble(),
                event.timestamp.toDouble() / 1_000_000.0 // ns to ms
            )
        }

    /**
     * Register for [sensorType], block until its first sample carrying at least
     * [minimumValues] values arrives, and shape it with [transform].
     * Returns the "unavailable" marker [0.0] when the sensor is missing or stays
     * silent for [EVENT_TIMEOUT_MS].
     */
    private fun awaitFirstEvent(
        context: Context,
        sensorType: Int,
        minimumValues: Int,
        transform: (SensorEvent) -> DoubleArray,
    ): DoubleArray {
        val manager = context.getSystemService(Context.SENSOR_SERVICE) as? SensorManager
            ?: return unavailable()

        val sensor = manager.getDefaultSensor(sensorType) ?: return unavailable()

        // The caller blocks until the first event arrives, so delivery must not
        // depend on a looper the caller may itself be blocking. Delivering on
        // the main looper made a read from the UI thread queue its own callback
        // behind the wait that callback has to satisfy, so it timed out every
        // time and reported a working sensor as unavailable.
        val deliveryThread = HandlerThread("waterkit-sensor")
        deliveryThread.start()

        try {
            var result: DoubleArray? = null
            val lock = Object()

            val listener = object : SensorEventListener {
                override fun onSensorChanged(event: SensorEvent) {
                    synchronized(lock) {
                        if (result == null && event.values.size >= minimumValues) {
                            result = transform(event)
                        }
                        lock.notifyAll()
                    }
                }

                override fun onAccuracyChanged(sensor: Sensor, accuracy: Int) {}
            }

            synchronized(lock) {
                manager.registerListener(
                    listener,
                    sensor,
                    SensorManager.SENSOR_DELAY_GAME,
                    Handler(deliveryThread.looper),
                )

                val deadline = SystemClock.uptimeMillis() + EVENT_TIMEOUT_MS
                while (result == null) {
                    val remaining = deadline - SystemClock.uptimeMillis()
                    if (remaining <= 0L) {
                        break
                    }
                    try {
                        lock.wait(remaining)
                    } catch (e: InterruptedException) {
                        Thread.currentThread().interrupt()
                        break
                    }
                }
            }

            manager.unregisterListener(listener)

            return synchronized(lock) { result } ?: unavailable()
        } finally {
            deliveryThread.quitSafely()
        }
    }

    /** The marker the Rust side decodes as [`SensorError::NotAvailable`]. */
    private fun unavailable() = doubleArrayOf(0.0)
}
