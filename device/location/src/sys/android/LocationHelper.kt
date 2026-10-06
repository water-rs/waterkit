package waterkit.location

import android.content.Context
import android.content.pm.PackageManager
import android.location.Location
import android.location.LocationListener
import android.location.LocationManager
import android.location.LocationRequest
import android.os.Build
import android.os.Looper
import com.google.android.gms.common.ConnectionResult
import com.google.android.gms.common.GoogleApiAvailability
import com.google.android.gms.location.CurrentLocationRequest
import com.google.android.gms.location.LocationServices
import com.google.android.gms.location.Priority
import com.google.android.gms.tasks.CancellationTokenSource
import com.google.android.gms.tasks.Task
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

/**
 * Helper class for requesting the device location on Android.
 * Compiled by the packager onto the application's classpath, with the
 * crate's declared `play-services-location` client.
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
     * Whether Google Play services is installed, enabled and recent enough
     * for the linked `play-services-location` client — the device's own
     * answer, asked once before the first request.
     */
    @JvmStatic
    fun hasGooglePlayServices(context: Context): Boolean =
        GoogleApiAvailability.getInstance().isGooglePlayServicesAvailable(context) ==
            ConnectionResult.SUCCESS

    /** The location permission the app holds, or `null` when it holds none. */
    private enum class Grant { FINE, COARSE }

    private fun grant(context: Context): Grant? = when {
        context.checkSelfPermission(android.Manifest.permission.ACCESS_FINE_LOCATION) ==
            PackageManager.PERMISSION_GRANTED -> Grant.FINE
        context.checkSelfPermission(android.Manifest.permission.ACCESS_COARSE_LOCATION) ==
            PackageManager.PERMISSION_GRANTED -> Grant.COARSE
        else -> null
    }

    /**
     * Requests a fresh fix from the Fused Location Provider of Google Play
     * services and blocks the calling thread until it arrives or
     * [timeoutMillis] elapses. The caller must not be the main thread.
     */
    @JvmStatic
    fun getFusedLocation(context: Context, timeoutMillis: Long): Result {
        val grant = grant(context) ?: return Result.failure(STATUS_PERMISSION_DENIED)
        val manager = context.getSystemService(LocationManager::class.java)
            ?: return Result.failure(STATUS_UNAVAILABLE)
        // The fused provider serves no fix while the device's location switch
        // is off; it answers `null`, which would read as "no fix" instead.
        if (!locationEnabled(manager)) {
            return Result.failure(STATUS_SERVICE_DISABLED)
        }

        val request = CurrentLocationRequest.Builder()
            .setPriority(
                when (grant) {
                    Grant.FINE -> Priority.PRIORITY_HIGH_ACCURACY
                    Grant.COARSE -> Priority.PRIORITY_BALANCED_POWER_ACCURACY
                },
            )
            .build()
        val cancellation = CancellationTokenSource()
        val completed = AtomicReference<Task<Location>?>(null)
        val latch = CountDownLatch(1)
        LocationServices.getFusedLocationProviderClient(context)
            .getCurrentLocation(request, cancellation.token)
            .addOnCompleteListener({ runnable -> runnable.run() }) { task ->
                completed.set(task)
                latch.countDown()
            }

        if (!latch.await(timeoutMillis, TimeUnit.MILLISECONDS)) {
            cancellation.cancel()
            return Result.failure(STATUS_TIMEOUT)
        }
        val task = completed.get()
            ?: throw IllegalStateException("fused location task completed without a result")
        if (task.isSuccessful) {
            return task.result?.let(Result::success) ?: Result.failure(STATUS_UNAVAILABLE)
        }
        return when (val exception = task.exception) {
            is SecurityException -> Result.failure(STATUS_PERMISSION_DENIED)
            // Any other Google Play services failure is not a status this crate
            // can name: it reaches Rust as the exception, on the caller's thread.
            null -> throw IllegalStateException("fused location task failed without an exception")
            else -> throw exception
        }
    }

    private fun locationEnabled(manager: LocationManager): Boolean =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            manager.isLocationEnabled
        } else {
            manager.isProviderEnabled(LocationManager.GPS_PROVIDER) ||
                manager.isProviderEnabled(LocationManager.NETWORK_PROVIDER)
        }

    /**
     * Requests a fresh fix from the framework `LocationManager` and blocks the
     * calling thread until it arrives or [timeoutMillis] elapses. Callbacks
     * are delivered on the main looper below API 30, so the caller must not be
     * the main thread.
     *
     * From API 31 the framework registers its own fused provider
     * (`com.android.location.fused`, a system app on AOSP builds, which fuses
     * GPS and network fixes and serves coarse grants); it is asked when the
     * device registers it. Otherwise GPS serves a fine grant while it is
     * enabled, and the network provider anything else.
     */
    @JvmStatic
    fun getFrameworkLocation(context: Context, timeoutMillis: Long): Result {
        val grant = grant(context) ?: return Result.failure(STATUS_PERMISSION_DENIED)
        val manager = context.getSystemService(LocationManager::class.java)
            ?: return Result.failure(STATUS_UNAVAILABLE)

        val provider = frameworkProvider(manager, grant)
            ?: return Result.failure(STATUS_SERVICE_DISABLED)

        val received = AtomicReference<Location?>(null)
        val latch = CountDownLatch(1)
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                // The quality steers `FUSED_PROVIDER` the way the priority
                // steers the Play services one: GNSS for a fine grant.
                val request = LocationRequest.Builder(0L)
                    .setQuality(
                        when (grant) {
                            Grant.FINE -> LocationRequest.QUALITY_HIGH_ACCURACY
                            Grant.COARSE -> LocationRequest.QUALITY_BALANCED_POWER_ACCURACY
                        },
                    )
                    .build()
                manager.getCurrentLocation(
                    provider,
                    request,
                    null,
                    { runnable -> runnable.run() },
                ) { location ->
                    received.set(location)
                    latch.countDown()
                }
            } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
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

    /** The enabled provider that serves [grant], or `null` when none is enabled. */
    private fun frameworkProvider(manager: LocationManager, grant: Grant): String? {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S &&
            manager.hasProvider(LocationManager.FUSED_PROVIDER)
        ) {
            return LocationManager.FUSED_PROVIDER.takeIf(manager::isProviderEnabled)
        }
        return when {
            grant == Grant.FINE && manager.isProviderEnabled(LocationManager.GPS_PROVIDER) ->
                LocationManager.GPS_PROVIDER
            manager.isProviderEnabled(LocationManager.NETWORK_PROVIDER) ->
                LocationManager.NETWORK_PROVIDER
            else -> null
        }
    }
}
