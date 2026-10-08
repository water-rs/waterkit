package waterkit.vision

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.IntentSender
import com.google.android.gms.common.ConnectionResult
import com.google.android.gms.common.GoogleApiAvailability
import com.google.mlkit.vision.documentscanner.GmsDocumentScannerOptions
import com.google.mlkit.vision.documentscanner.GmsDocumentScanning
import com.google.mlkit.vision.documentscanner.GmsDocumentScanningResult

/**
 * Helper class for the one-shot system document scanner.
 * Compiled by the packager onto the application's classpath, with the
 * crate's declared `play-services-mlkit-document-scanner` client. The
 * scanning UI itself is Google Play services' module, so the app needs no
 * camera permission.
 */
object DocumentScannerHelper {
    /**
     * Whether Google Play services is installed, enabled and recent enough
     * for the linked `play-services-mlkit-document-scanner` client — the
     * device's own answer, asked before a scan and by `capabilities()`.
     * Kept self-contained: each feature's sources compile independently.
     */
    @JvmStatic
    fun hasGooglePlayServices(context: Context): Boolean =
        GoogleApiAvailability.getInstance().isGooglePlayServicesAvailable(context) ==
            ConnectionResult.SUCCESS

    /**
     * Builds the document scanner's launch `IntentSender` — honoring
     * [pageLimit] (`0` leaves the scanner's own default) and
     * [galleryImport] — and hands it to Rust through [onScanIntent]: a
     * sender on success (launched through the waterkit-build
     * activity-result bridge), a cancel (null sender, null error) or a
     * failure (non-null error).
     */
    @JvmStatic
    fun scan(context: Context, requestId: Long, pageLimit: Int, galleryImport: Boolean) {
        require(requestId > 0) { "invalid document scan request id: $requestId" }
        require(pageLimit >= 0) { "invalid page limit: $pageLimit" }
        val activity = context as? Activity
        if (activity == null) {
            onScanIntent(
                requestId,
                null,
                "the published Context is not an Activity; the document scanner cannot launch",
            )
            return
        }
        val builder =
            GmsDocumentScannerOptions.Builder()
                .setGalleryImportAllowed(galleryImport)
                .setResultFormats(GmsDocumentScannerOptions.RESULT_FORMAT_JPEG)
                .setScannerMode(GmsDocumentScannerOptions.SCANNER_MODE_FULL)
        if (pageLimit > 0) {
            builder.setPageLimit(pageLimit)
        }
        GmsDocumentScanning.getClient(builder.build())
            .getStartScanIntent(activity)
            .addOnSuccessListener { sender -> onScanIntent(requestId, sender, null) }
            .addOnCanceledListener { onScanIntent(requestId, null, null) }
            .addOnFailureListener { error ->
                onScanIntent(requestId, null, error.message ?: error.javaClass.name)
            }
    }

    /**
     * Reads the scanned pages out of the scanner's result [Intent] as JPEG
     * bytes, in scan order.
     */
    @JvmStatic
    fun pageImages(context: Context, data: Intent): Array<ByteArray> {
        val scan =
            GmsDocumentScanningResult.fromActivityResultIntent(data)
                ?: error("the document scanner returned an unreadable result intent")
        val pages = scan.pages ?: error("the document scanner returned no pages")
        return Array(pages.size) { index ->
            val uri = pages[index].imageUri
            context.contentResolver.openInputStream(uri)?.use { stream -> stream.readBytes() }
                ?: error("could not open scanned page $uri")
        }
    }

    @JvmStatic
    private external fun onScanIntent(
        requestId: Long,
        sender: IntentSender?,
        error: String?,
    )
}
