package waterkit.vision

import android.app.Activity
import android.content.Context
import android.content.Intent
import com.google.mlkit.vision.documentscanner.GmsDocumentScannerOptions
import com.google.mlkit.vision.documentscanner.GmsDocumentScanning
import com.google.mlkit.vision.documentscanner.GmsDocumentScanningResult
import waterkit.build.NativeCallback

/**
 * Helper class for the one-shot system document scanner.
 * Compiled by the packager onto the application's classpath, with the
 * crate's declared `play-services-mlkit-document-scanner` client. The
 * scanning UI itself is Google Play services' module, so the app needs no
 * camera permission.
 */
object DocumentScannerHelper {
    /**
     * Builds the document scanner's launch `IntentSender` — honoring
     * [pageLimit] (`0` leaves the scanner's own default) and
     * [galleryImport] — and completes [callback] with it on success
     * (launched through the waterkit-build activity-result bridge), with
     * `null` when the intent task cancelled, or reports the failure through
     * [NativeCallback.fail].
     */
    @JvmStatic
    fun scan(context: Context, callback: NativeCallback, pageLimit: Int, galleryImport: Boolean) {
        require(pageLimit >= 0) { "invalid page limit: $pageLimit" }
        val activity = context as? Activity
        if (activity == null) {
            callback.fail(
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
            .addOnSuccessListener { sender -> callback.complete(sender) }
            .addOnCanceledListener { callback.complete(null) }
            .addOnFailureListener { error ->
                callback.fail(error.message ?: error.javaClass.name)
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
}
