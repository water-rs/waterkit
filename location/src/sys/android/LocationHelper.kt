package waterkit.location

import android.content.Context
import android.location.Location
import android.location.LocationManager

/**
 * Helper class for accessing location on Android.
 * Compiled to DEX and embedded in the Rust library.
 */
object LocationHelper {
    
    /**
     * Get the last known location from any available provider.
     * Returns array: [success, latitude, longitude, altitude, accuracy, timestamp]
     * On failure: [0.0]
     */
    @JvmStatic
    fun getLastKnownLocation(context: Context): DoubleArray {
        val manager = context.getSystemService(Context.LOCATION_SERVICE) as? LocationManager
            ?: return doubleArrayOf(0.0)

        val location = tryGetLocation(manager, LocationManager.GPS_PROVIDER)
            ?: tryGetLocation(manager, LocationManager.NETWORK_PROVIDER)
            ?: return doubleArrayOf(0.0)

        return doubleArrayOf(
            1.0, // success
            location.latitude,
            location.longitude,
            location.altitude,
            location.accuracy.toDouble(),
            location.time.toDouble()
        )
    }

    @Suppress("MissingPermission")
    private fun tryGetLocation(manager: LocationManager, provider: String): Location? {
        return try {
            manager.getLastKnownLocation(provider)
        } catch (e: SecurityException) {
            null
        }
    }
}
