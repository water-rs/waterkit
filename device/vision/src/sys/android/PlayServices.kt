package waterkit.vision

import android.content.Context
import com.google.android.gms.common.ConnectionResult
import com.google.android.gms.common.GoogleApiAvailability

/**
 * The Google Play services probe the scanner, barcode and text
 * realizations share. Compiled by the packager whenever any of the three
 * features is enabled; the crate declares it in each `feature.*`
 * kotlin-sources list, where it deduplicates by name.
 */
object PlayServices {
    @JvmStatic
    fun hasGooglePlayServices(context: Context): Boolean =
        GoogleApiAvailability.getInstance().isGooglePlayServicesAvailable(context) ==
            ConnectionResult.SUCCESS
}
