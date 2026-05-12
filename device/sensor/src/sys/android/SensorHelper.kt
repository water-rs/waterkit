package waterkit.sensor

import android.content.Context
import android.hardware.Sensor
import android.hardware.SensorEvent
import android.hardware.SensorEventListener
import android.hardware.SensorManager
import android.os.Handler
import android.os.Looper

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
    fun readSensor(context: Context, sensorType: Int): DoubleArray {
        val manager = context.getSystemService(Context.SENSOR_SERVICE) as? SensorManager
            ?: return doubleArrayOf(0.0)

        val sensor = manager.getDefaultSensor(sensorType)
            ?: return doubleArrayOf(0.0)

        var result: DoubleArray? = null
        val lock = Object()

        val listener = object : SensorEventListener {
            override fun onSensorChanged(event: SensorEvent) {
                if (event.values.size >= 3) {
                    result = doubleArrayOf(
                        1.0, // success
                        event.values[0].toDouble(),
                        event.values[1].toDouble(),
                        event.values[2].toDouble(),
                        event.timestamp.toDouble() / 1_000_000.0 // ns to ms
                    )
                }
                synchronized(lock) {
                    lock.notify()
                }
            }

            override fun onAccuracyChanged(sensor: Sensor, accuracy: Int) {}
        }

        val handler = Handler(Looper.getMainLooper())
        manager.registerListener(listener, sensor, SensorManager.SENSOR_DELAY_GAME, handler)

        synchronized(lock) {
            try {
                lock.wait(1000) // 1 second timeout
            } catch (e: InterruptedException) {
                // Ignored
            }
        }

        manager.unregisterListener(listener)

        return result ?: doubleArrayOf(0.0)
    }

    /**
     * Read pressure sensor (barometer).
     * Returns array: [success, pressure_hPa, timestamp]
     * On failure: [0.0]
     */
    @JvmStatic
    fun readPressure(context: Context): DoubleArray {
        val manager = context.getSystemService(Context.SENSOR_SERVICE) as? SensorManager
            ?: return doubleArrayOf(0.0)

        val sensor = manager.getDefaultSensor(Sensor.TYPE_PRESSURE)
            ?: return doubleArrayOf(0.0)

        var result: DoubleArray? = null
        val lock = Object()

        val listener = object : SensorEventListener {
            override fun onSensorChanged(event: SensorEvent) {
                if (event.values.isNotEmpty()) {
                    result = doubleArrayOf(
                        1.0, // success
                        event.values[0].toDouble(), // pressure in hPa
                        event.timestamp.toDouble() / 1_000_000.0 // ns to ms
                    )
                }
                synchronized(lock) {
                    lock.notify()
                }
            }

            override fun onAccuracyChanged(sensor: Sensor, accuracy: Int) {}
        }

        val handler = Handler(Looper.getMainLooper())
        manager.registerListener(listener, sensor, SensorManager.SENSOR_DELAY_GAME, handler)

        synchronized(lock) {
            try {
                lock.wait(1000)
            } catch (e: InterruptedException) {
                // Ignored
            }
        }

        manager.unregisterListener(listener)

        return result ?: doubleArrayOf(0.0)
        return result ?: doubleArrayOf(0.0)
    }

    /**
     * Read ambient light sensor.
     * Returns array: [success, lux, timestamp]
     * On failure: [0.0]
     */
    @JvmStatic
    fun readLight(context: Context): DoubleArray {
        val manager = context.getSystemService(Context.SENSOR_SERVICE) as? SensorManager
            ?: return doubleArrayOf(0.0)

        val sensor = manager.getDefaultSensor(Sensor.TYPE_LIGHT)
            ?: return doubleArrayOf(0.0)

        var result: DoubleArray? = null
        val lock = Object()

        val listener = object : SensorEventListener {
            override fun onSensorChanged(event: SensorEvent) {
                if (event.values.isNotEmpty()) {
                    result = doubleArrayOf(
                        1.0, // success
                        event.values[0].toDouble(), // light in lux
                        event.timestamp.toDouble() / 1_000_000.0 // ns to ms
                    )
                }
                synchronized(lock) {
                    lock.notify()
                }
            }

            override fun onAccuracyChanged(sensor: Sensor, accuracy: Int) {}
        }

        val handler = Handler(Looper.getMainLooper())
        manager.registerListener(listener, sensor, SensorManager.SENSOR_DELAY_GAME, handler)

        synchronized(lock) {
            try {
                lock.wait(1000)
            } catch (e: InterruptedException) {
                // Ignored
            }
        }

        manager.unregisterListener(listener)

        return result ?: doubleArrayOf(0.0)
    }
}
