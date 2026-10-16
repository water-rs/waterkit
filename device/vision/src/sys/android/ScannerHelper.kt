package waterkit.vision

import android.content.Context
import com.google.mlkit.vision.codescanner.GmsBarcodeScannerOptions
import com.google.mlkit.vision.codescanner.GmsBarcodeScanning
import waterkit.build.NativeCallback

/**
 * Helper class for the one-shot system code scanner.
 * Compiled by the packager onto the application's classpath, with the
 * crate's declared `play-services-code-scanner` client. The scanning UI
 * itself is Google Play services' module, so the app needs no camera
 * permission.
 */
object ScannerHelper {
    /**
     * Presents the Google code scanner restricted to [formats]
     * (`Barcode#FORMAT_*` values) with auto-zoom on, and completes
     * [callback] with the decoded `Barcode`, a `null` cancel, or reports the
     * failure through [NativeCallback.fail].
     */
    @JvmStatic
    fun scan(context: Context, callback: NativeCallback, formats: IntArray) {
        require(formats.isNotEmpty()) { "a scan needs at least one format" }
        val options =
            GmsBarcodeScannerOptions.Builder()
                .setBarcodeFormats(formats[0], *formats.copyOfRange(1, formats.size))
                .enableAutoZoom()
                .build()
        GmsBarcodeScanning.getClient(context, options)
            .startScan()
            .addOnSuccessListener { barcode -> callback.complete(barcode) }
            .addOnCanceledListener { callback.complete(null) }
            .addOnFailureListener { error ->
                callback.fail(error.message ?: error.javaClass.name)
            }
    }
}
