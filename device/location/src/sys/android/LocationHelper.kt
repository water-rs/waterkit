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
import waterkit.build.NativeCallback

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
     * services. The task's success and failure listeners complete
     * [callback] on the thread Play services answers on, so no thread is
     * parked waiting.
     */
    @JvmStatic
    fun getFusedLocation(context: Context, callback: NativeCallback) {
        val grant = grant(context)
        if (grant == null) {
            callback.complete(Result.failure(STATUS_PERMISSION_DENIED))
            return
        }
        val manager = context.getSystemService(LocationManager::class.java)
        if (manager == null) {
            callback.complete(Result.failure(STATUS_UNAVAILABLE))
            return
        }
        // The fused provider serves no fix while the device's location switch
        // is off; it answers `null`, which would read as "no fix" instead.
        if (!locationEnabled(manager)) {
            callback.complete(Result.failure(STATUS_SERVICE_DISABLED))
            return
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
        val task = LocationServices.getFusedLocationProviderClient(context)
            .getCurrentLocation(request, cancellation.token)
        task.addOnSuccessListener({ runnable -> runnable.run() }) { location ->
            callback.complete(
                location?.let(Result::success) ?: Result.failure(STATUS_UNAVAILABLE),
            )
        }
        task.addOnFailureListener({ runnable -> runnable.run() }) { exception ->
            when (exception) {
                is SecurityException ->
                    callback.complete(Result.failure(STATUS_PERMISSION_DENIED))
                // Any other Google Play services failure is not a status this
                // crate can name: it reaches Rust as a rejection.
                else -> callback.fail(exception.toString())
            }
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
     * Requests a fresh fix from the framework `LocationManager`; the
     * platform's listener completes [callback]. Callbacks are delivered on
     * the main looper below API 30, and on the provider's answer thread
     * from API 30.
     *
     * From API 31 the framework registers its own fused provider
     * (`com.android.location.fused`, a system app on AOSP builds, which fuses
     * GPS and network fixes and serves coarse grants); it is asked when the
     * device registers it. Otherwise GPS serves a fine grant while it is
     * enabled, and the network provider anything else.
     */
    @JvmStatic
    fun getFrameworkLocation(context: Context, callback: NativeCallback) {
        val grant = grant(context)
        if (grant == null) {
            callback.complete(Result.failure(STATUS_PERMISSION_DENIED))
            return
        }
        val manager = context.getSystemService(LocationManager::class.java)
        if (manager == null) {
            callback.complete(Result.failure(STATUS_UNAVAILABLE))
            return
        }

        val provider = frameworkProvider(manager, grant)
        if (provider == null) {
            callback.complete(Result.failure(STATUS_SERVICE_DISABLED))
            return
        }

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
                    callback.complete(
                        location?.let(Result::success) ?: Result.failure(STATUS_UNAVAILABLE),
                    )
                }
            } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                manager.getCurrentLocation(
                    provider,
                    null,
                    { runnable -> runnable.run() },
                ) { location ->
                    callback.complete(
                        location?.let(Result::success) ?: Result.failure(STATUS_UNAVAILABLE),
                    )
                }
            } else {
                @Suppress("DEPRECATION") // getCurrentLocation requires API 30.
                manager.requestSingleUpdate(
                    provider,
                    LocationListener { location ->
                        callback.complete(Result.success(location))
                    },
                    Looper.getMainLooper(),
                )
            }
        } catch (e: SecurityException) {
            callback.complete(Result.failure(STATUS_PERMISSION_DENIED))
        }
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
