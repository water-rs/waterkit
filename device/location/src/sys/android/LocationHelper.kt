package waterkit.location

import android.content.Context
import android.content.pm.PackageManager
import android.location.Location
import android.location.LocationListener
import android.location.LocationManager
import android.os.Build
import android.os.Looper
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

/**
 * Helper class for requesting the device location on Android.
 * Compiled to DEX and embedded in the Rust library.
 */
object LocationHelper {
    const val STATUS_SUCCESS = 0
    const val STATUS_PERMISSION_DENIED = 1
    const val STATUS_SERVICE_DISABLED = 2
    const val STATUS_UNAVAILABLE = 3
    const val STATUS_TIMEOUT = 4

    /** Typed result read field-by-field from Rust over JNI. */
    class Result(
        @JvmField val status: Int,
        @JvmField val latitude: Double,
        @JvmField val longitude: Double,
        @JvmField val hasAltitude: Boolean,
        @JvmField val altitude: Double,
        @JvmField val hasHorizontalAccuracy: Boolean,
        @JvmField val horizontalAccuracy: Double,
        @JvmField val hasVerticalAccuracy: Boolean,
        @JvmField val verticalAccuracy: Double,
        @JvmField val timeMillis: Long,
    ) {
        companion object {
            fun failure(status: Int): Result =
                Result(status, 0.0, 0.0, false, 0.0, false, 0.0, false, 0.0, 0L)

            fun success(location: Location): Result {
                val hasVertical =
                    Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && location.hasVerticalAccuracy()
                return Result(
                    STATUS_SUCCESS,
                    location.latitude,
                    location.longitude,
                    location.hasAltitude(),
                    location.altitude,
                    location.hasAccuracy(),
                    location.accuracy.toDouble(),
                    hasVertical,
                    if (hasVertical) location.verticalAccuracyMeters.toDouble() else 0.0,
                    location.time,
                )
            }
        }
    }

    /**
     * Requests a fresh location fix and blocks the calling thread until the fix
     * arrives or [timeoutMillis] elapses. Callbacks are delivered on the main
     * looper, so the caller must not be the main thread.
     */
    @JvmStatic
    fun getCurrentLocation(context: Context, timeoutMillis: Long): Result {
        val fine =
            context.checkSelfPermission(android.Manifest.permission.ACCESS_FINE_LOCATION) ==
                PackageManager.PERMISSION_GRANTED
        val coarse =
            context.checkSelfPermission(android.Manifest.permission.ACCESS_COARSE_LOCATION) ==
                PackageManager.PERMISSION_GRANTED
        if (!fine && !coarse) {
            return Result.failure(STATUS_PERMISSION_DENIED)
        }

        val manager = context.getSystemService(Context.LOCATION_SERVICE) as? LocationManager
            ?: return Result.failure(STATUS_UNAVAILABLE)

        val provider = when {
            fine && manager.isProviderEnabled(LocationManager.GPS_PROVIDER) ->
                LocationManager.GPS_PROVIDER
            manager.isProviderEnabled(LocationManager.NETWORK_PROVIDER) ->
                LocationManager.NETWORK_PROVIDER
            else -> return Result.failure(STATUS_SERVICE_DISABLED)
        }

        val received = AtomicReference<Location?>(null)
        val latch = CountDownLatch(1)
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                manager.getCurrentLocation(
                    provider,
                    null,
                    { runnable -> runnable.run() },
                ) { location ->
                    received.set(location)
                    latch.countDown()
                }
            } else {
                @Suppress("DEPRECATION") // getCurrentLocation requires API 30.
                manager.requestSingleUpdate(
                    provider,
                    LocationListener { location ->
                        received.set(location)
                        latch.countDown()
                    },
                    Looper.getMainLooper(),
                )
            }
        } catch (e: SecurityException) {
            return Result.failure(STATUS_PERMISSION_DENIED)
        }

        if (!latch.await(timeoutMillis, TimeUnit.MILLISECONDS)) {
            return Result.failure(STATUS_TIMEOUT)
        }
        val location = received.get() ?: return Result.failure(STATUS_UNAVAILABLE)
        return Result.success(location)
    }
}
