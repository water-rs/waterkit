package waterkit.vision

import android.content.Context
import com.google.android.gms.common.moduleinstall.ModuleInstall
import com.google.android.gms.common.moduleinstall.ModuleInstallRequest
import com.google.android.gms.tasks.Tasks
import com.google.mlkit.vision.barcode.BarcodeScanner
import com.google.mlkit.vision.barcode.BarcodeScannerOptions
import com.google.mlkit.vision.barcode.BarcodeScanning
import com.google.mlkit.vision.common.InputImage
import java.util.concurrent.TimeUnit

/**
 * Kotlin half of `waterkit-vision`'s `barcode` feature on Android: thin
 * wrappers over the unbundled ML Kit barcode client. The engine lives in a
 * Play services module `prepareModule` installs on demand.
 *
 * Every method that touches a Play services `Task` blocks on `Tasks.await`
 * and must run on the dedicated threads the Rust side spawns for it.
 */
object VisionBarcodeHelper {
    // The one module this helper installs; `prepareModule`'s second argument
    // is kept so its JNI signature matches `VisionTextHelper`'s.
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
     * Downloads the barcode module when it is not installed, returning once
     * it is. Any failure throws, which the Rust side reports as
     * model-unavailable.
     */
    @JvmStatic
    fun prepareModule(context: Context, module: Int) {
        require(module == MODULE_BARCODE) { "unknown barcode module $module" }
        val client = ModuleInstall.getClient(context)
        val api = BarcodeScanning.getClient()
        val installed = Tasks.await(
            client.areModulesAvailable(api),
            MlKitInput.DETECTION_TIMEOUT_MS,
            TimeUnit.MILLISECONDS,
        ).areModulesAvailable()
        if (installed) return
        Tasks.await(
            client.installModules(ModuleInstallRequest.newBuilder().addApi(api).build()),
            MlKitInput.INSTALL_TIMEOUT_MS,
            TimeUnit.MILLISECONDS,
        )
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

    /** Every barcode in `input` matching `formats`, in the image's stored orientation. */
    @JvmStatic
    fun detectBarcodes(input: InputImage, formats: IntArray): Array<BarcodeRow> {
        val found = Tasks.await(
            scannerFor(formats).process(input),
            MlKitInput.DETECTION_TIMEOUT_MS,
            TimeUnit.MILLISECONDS,
        )
        return found.map { barcode ->
            BarcodeRow(
                barcode.format,
                barcode.rawBytes ?: barcode.rawValue?.encodeToByteArray() ?: ByteArray(0),
                MlKitInput.corners(barcode.cornerPoints, barcode.boundingBox),
            )
        }.toTypedArray()
    }
}
