package waterkit.vision

import android.content.Context
import com.google.mlkit.vision.codescanner.GmsBarcodeScannerOptions
import com.google.mlkit.vision.codescanner.GmsBarcodeScanning

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
     * (`Barcode#FORMAT_*` values) with auto-zoom on, and reports the outcome
     * through [onScanResult]: a decoded payload and its format, a cancel
     * (null payload, null error), or a failure (non-null error).
     */
    @JvmStatic
    fun scan(context: Context, requestId: Long, formats: IntArray) {
        require(requestId > 0) { "invalid scan request id: $requestId" }
        require(formats.isNotEmpty()) { "a scan needs at least one format" }
        val options =
            GmsBarcodeScannerOptions.Builder()
                .setBarcodeFormats(formats[0], *formats.copyOfRange(1, formats.size))
                .enableAutoZoom()
                .build()
        GmsBarcodeScanning.getClient(context, options)
            .startScan()
            .addOnSuccessListener { barcode ->
                onScanResult(requestId, barcode.rawBytes, barcode.format, null)
            }
            .addOnCanceledListener {
                onScanResult(requestId, null, 0, null)
            }
            .addOnFailureListener { error ->
                onScanResult(requestId, null, 0, error.message ?: error.javaClass.name)
            }
    }

    @JvmStatic
    private external fun onScanResult(
        requestId: Long,
        payload: ByteArray?,
        format: Int,
        error: String?,
    )
}
