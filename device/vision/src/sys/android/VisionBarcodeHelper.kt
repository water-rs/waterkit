package waterkit.vision

import android.content.Context
import com.google.android.gms.common.moduleinstall.ModuleInstall
import com.google.android.gms.common.moduleinstall.ModuleInstallRequest
import com.google.mlkit.vision.barcode.BarcodeScanner
import com.google.mlkit.vision.barcode.BarcodeScannerOptions
import com.google.mlkit.vision.barcode.BarcodeScanning
import com.google.mlkit.vision.common.InputImage
import waterkit.build.NativeCallback

/**
 * Kotlin half of `waterkit-vision`'s `barcode` feature on Android: thin
 * wrappers over the unbundled ML Kit barcode client. The engine lives in a
 * Play services module `prepareModule` installs on demand.
 *
 * Every method that touches a Play services `Task` answers through the
 * `NativeCallback` it is handed, from the task's listeners; nothing blocks.
 */
object VisionBarcodeHelper {
    // The one module this helper installs.
    private const val MODULE_BARCODE = 0

    /** A detected barcode, flattened for field-by-field reads over JNI. */
    class BarcodeRow(
        @JvmField val format: Int,
        @JvmField val bytes: ByteArray,
        @JvmField val points: IntArray,
    )

    // One scanner per requested format set; the engine itself lives in Play
    // services' dynamite module.
    private val scanners = HashMap<Int, BarcodeScanner>()

    /**
     * Downloads the barcode module when it is not installed. [callback]
     * completes with `null` once the module is available, and fails with the
     * probe's or the install's error.
     */
    @JvmStatic
    fun prepareModule(context: Context, callback: NativeCallback, module: Int) {
        require(module == MODULE_BARCODE) { "unknown barcode module $module" }
        val client = ModuleInstall.getClient(context)
        val api = BarcodeScanning.getClient()
        client.areModulesAvailable(api)
            .addOnSuccessListener { availability ->
                if (availability.areModulesAvailable()) {
                    callback.complete(null)
                } else {
                    client.installModules(
                        ModuleInstallRequest.newBuilder().addApi(api).build(),
                    )
                        .addOnSuccessListener { callback.complete(null) }
                        .addOnCanceledListener {
                            callback.fail("the barcode module install was cancelled")
                        }
                        .addOnFailureListener { error ->
                            callback.fail(error.message ?: error.javaClass.name)
                        }
                }
            }
            .addOnCanceledListener {
                callback.fail("the barcode module probe was cancelled")
            }
            .addOnFailureListener { error ->
                callback.fail(error.message ?: error.javaClass.name)
            }
    }

    private fun scannerFor(formats: IntArray): BarcodeScanner =
        synchronized(scanners) {
            scanners.getOrPut(formats.contentHashCode()) {
                BarcodeScanning.getClient(
                    BarcodeScannerOptions.Builder()
                        .setBarcodeFormats(formats[0], *formats.copyOfRange(1, formats.size))
                        .build(),
                )
            }
        }

    /**
     * Completes [callback] with every barcode in [input] matching [formats],
     * in the image's stored orientation.
     */
    @JvmStatic
    fun detectBarcodes(input: InputImage, callback: NativeCallback, formats: IntArray) {
        scannerFor(formats).process(input)
            .addOnSuccessListener { found ->
                callback.complete(
                    found.map { barcode ->
                        BarcodeRow(
                            barcode.format,
                            barcode.rawBytes ?: barcode.rawValue?.encodeToByteArray()
                                ?: ByteArray(0),
                            MlKitInput.corners(barcode.cornerPoints, barcode.boundingBox),
                        )
                    }.toTypedArray(),
                )
            }
            .addOnCanceledListener { callback.fail("barcode detection was cancelled") }
            .addOnFailureListener { error ->
                callback.fail(error.message ?: error.javaClass.name)
            }
    }
}
