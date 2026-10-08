package waterkit.vision

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Matrix
import android.graphics.Point
import android.graphics.Rect
import android.media.ExifInterface
import com.google.android.gms.common.ConnectionResult
import com.google.android.gms.common.GoogleApiAvailability
import com.google.android.gms.common.api.OptionalModuleApi
import com.google.android.gms.common.moduleinstall.ModuleInstall
import com.google.android.gms.common.moduleinstall.ModuleInstallRequest
import com.google.android.gms.tasks.Tasks
import com.google.mlkit.vision.barcode.BarcodeScanner
import com.google.mlkit.vision.barcode.BarcodeScannerOptions
import com.google.mlkit.vision.barcode.BarcodeScanning
import com.google.mlkit.vision.common.InputImage
import com.google.mlkit.vision.text.TextRecognition
import com.google.mlkit.vision.text.TextRecognizer
import com.google.mlkit.vision.text.TextRecognizerOptionsInterface
import com.google.mlkit.vision.text.chinese.ChineseTextRecognizerOptions
import com.google.mlkit.vision.text.devanagari.DevanagariTextRecognizerOptions
import com.google.mlkit.vision.text.japanese.JapaneseTextRecognizerOptions
import com.google.mlkit.vision.text.korean.KoreanTextRecognizerOptions
import com.google.mlkit.vision.text.latin.TextRecognizerOptions
import java.io.ByteArrayInputStream
import java.util.concurrent.TimeUnit

/**
 * Kotlin half of `waterkit-vision` on Android: thin wrappers over the
 * unbundled ML Kit clients plus the platform bits they need (module
 * install, image decode, bitmap construction). Compiled by the packager
 * onto the application's classpath; the crate declares the unbundled
 * `play-services-mlkit-*` clients in its `maven` metadata.
 *
 * Every method that touches a Play services `Task` blocks on `Tasks.await`
 * and must run on the dedicated threads the Rust side spawns for it.
 */
object VisionHelper {
    // Module codes mirrored in `sys/android/mlkit.rs`: what `prepareModule`
    // and `recognizeText` take.
    private const val MODULE_BARCODE = 0
    private const val MODULE_LATIN = 1
    private const val MODULE_CHINESE = 2
    private const val MODULE_DEVANAGARI = 3
    private const val MODULE_JAPANESE = 4
    private const val MODULE_KOREAN = 5

    private const val DETECTION_TIMEOUT_MS = 30_000L
    private const val INSTALL_TIMEOUT_MS = 120_000L

    /** A detected barcode, flattened for field-by-field reads over JNI. */
    class BarcodeRow(
        @JvmField val format: Int,
        @JvmField val bytes: ByteArray,
        @JvmField val points: IntArray,
    )

    /**
     * A recognized line of text, flattened for field-by-field reads over
     * JNI. `wordTexts`, `wordConfidences` and `wordPoints` are parallel:
     * entry `i` is the line's `i`th element, whose corner points are the
     * flat `x,y × 4` array `wordPoints[i]`.
     */
    class TextRow(
        @JvmField val text: String,
        @JvmField val confidence: Float,
        @JvmField val points: IntArray,
        @JvmField val wordTexts: Array<String>,
        @JvmField val wordConfidences: FloatArray,
        @JvmField val wordPoints: Array<IntArray>,
    )

    // One scanner per requested format set, and one recognizer per script;
    // the engines themselves live in Play services' dynamite modules.
    private val scanners = HashMap<Int, BarcodeScanner>()
    private val recognizers = HashMap<Int, TextRecognizer>()

    @JvmStatic
    fun hasGooglePlayServices(context: Context): Boolean =
        GoogleApiAvailability.getInstance().isGooglePlayServicesAvailable(context) ==
            ConnectionResult.SUCCESS

    /** The `OptionalModuleApi` identifying each module's installable unit. */
    private fun moduleApi(module: Int): OptionalModuleApi = when (module) {
        MODULE_BARCODE -> BarcodeScanning.getClient()
        MODULE_LATIN -> TextRecognition.getClient(TextRecognizerOptions.Builder().build())
        MODULE_CHINESE -> TextRecognition.getClient(
            ChineseTextRecognizerOptions.Builder().build(),
        )
        MODULE_DEVANAGARI -> TextRecognition.getClient(
            DevanagariTextRecognizerOptions.Builder().build(),
        )
        MODULE_JAPANESE -> TextRecognition.getClient(
            JapaneseTextRecognizerOptions.Builder().build(),
        )
        else -> TextRecognition.getClient(KoreanTextRecognizerOptions.Builder().build())
    }

    /**
     * Downloads `module` when it is not installed, returning once it is.
     * Any failure throws, which the Rust side reports as model-unavailable.
     */
    @JvmStatic
    fun prepareModule(context: Context, module: Int) {
        val client = ModuleInstall.getClient(context)
        val api = moduleApi(module)
        val installed = Tasks.await(
            client.areModulesAvailable(api),
            DETECTION_TIMEOUT_MS,
            TimeUnit.MILLISECONDS,
        ).areModulesAvailable()
        if (installed) return
        Tasks.await(
            client.installModules(ModuleInstallRequest.newBuilder().addApi(api).build()),
            INSTALL_TIMEOUT_MS,
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

    private fun recognizerFor(script: Int): TextRecognizer =
        synchronized(recognizers) {
            recognizers.getOrPut(script) {
                val options: TextRecognizerOptionsInterface = when (script) {
                    MODULE_CHINESE -> ChineseTextRecognizerOptions.Builder().build()
                    MODULE_DEVANAGARI -> DevanagariTextRecognizerOptions.Builder().build()
                    MODULE_JAPANESE -> JapaneseTextRecognizerOptions.Builder().build()
                    MODULE_KOREAN -> KoreanTextRecognizerOptions.Builder().build()
                    else -> TextRecognizerOptions.Builder().build()
                }
                TextRecognition.getClient(options)
            }
        }

    /** Flattened corner points, or the bounding box's corners when null. */
    private fun corners(points: Array<Point>?, box: Rect?): IntArray {
        if (points != null && points.size == 4) {
            return intArrayOf(
                points[0].x, points[0].y,
                points[1].x, points[1].y,
                points[2].x, points[2].y,
                points[3].x, points[3].y,
            )
        }
        if (box != null) {
            return intArrayOf(
                box.left, box.top,
                box.right, box.top,
                box.right, box.bottom,
                box.left, box.bottom,
            )
        }
        return IntArray(0)
    }

    /** Every barcode in `input` matching `formats`, in the image's stored orientation. */
    @JvmStatic
    fun detectBarcodes(input: InputImage, formats: IntArray): Array<BarcodeRow> {
        val found = Tasks.await(
            scannerFor(formats).process(input),
            DETECTION_TIMEOUT_MS,
            TimeUnit.MILLISECONDS,
        )
        return found.map { barcode ->
            BarcodeRow(
                barcode.format,
                barcode.rawBytes ?: barcode.rawValue?.encodeToByteArray() ?: ByteArray(0),
                corners(barcode.cornerPoints, barcode.boundingBox),
            )
        }.toTypedArray()
    }

    /** Every line of text in `input`, with its elements, in the image's stored orientation. */
    @JvmStatic
    fun recognizeText(input: InputImage, script: Int): Array<TextRow> {
        val text = Tasks.await(
            recognizerFor(script).process(input),
            DETECTION_TIMEOUT_MS,
            TimeUnit.MILLISECONDS,
        )
        return text.textBlocks.flatMap { block -> block.lines }.map { line ->
            TextRow(
                line.text,
                line.confidence,
                corners(line.cornerPoints, line.boundingBox),
                line.elements.map { it.text }.toTypedArray(),
                FloatArray(line.elements.size) { line.elements[it].confidence },
                line.elements.map { corners(it.cornerPoints, it.boundingBox) }.toTypedArray(),
            )
        }.toTypedArray()
    }

    /** Wraps an `NV21` luma buffer for ML Kit; chroma may be neutral. */
    @JvmStatic
    fun nv21Input(bytes: ByteArray, width: Int, height: Int, rotationDegrees: Int): InputImage =
        InputImage.fromByteArray(bytes, width, height, rotationDegrees, InputImage.IMAGE_FORMAT_NV21)

    /** Wraps an already-upright `Bitmap` for ML Kit. */
    @JvmStatic
    fun bitmapInput(bitmap: Bitmap): InputImage = InputImage.fromBitmap(bitmap, 0)

    /** Decodes image bytes to an upright `Bitmap`, honoring EXIF orientation. */
    @JvmStatic
    fun decode(bytes: ByteArray): Bitmap {
        val bitmap = BitmapFactory.decodeByteArray(bytes, 0, bytes.size)
            ?: throw IllegalArgumentException("image bytes do not decode")
        val exif = runCatching {
            ExifInterface(ByteArrayInputStream(bytes)).getAttributeInt(
                ExifInterface.TAG_ORIENTATION,
                ExifInterface.ORIENTATION_NORMAL,
            )
        }.getOrDefault(ExifInterface.ORIENTATION_NORMAL)
        return upright(bitmap, exif)
    }

    /** Builds an upright `Bitmap` from tightly packed RGBA bytes. */
    @JvmStatic
    fun rgbaBitmap(rgba: ByteArray, width: Int, height: Int, exifOrientation: Int): Bitmap {
        require(rgba.size == width * height * 4) {
            "RGBA buffer ${rgba.size} != ${width}x${height}x4"
        }
        val bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888)
        bitmap.copyPixelsFromBuffer(java.nio.ByteBuffer.wrap(rgba))
        return upright(bitmap, exifOrientation)
    }

    /** Returns `bitmap` transformed to upright when `exif` rotates or mirrors it. */
    private fun upright(bitmap: Bitmap, exif: Int): Bitmap {
        val matrix = Matrix()
        when (exif) {
            ExifInterface.ORIENTATION_FLIP_HORIZONTAL -> matrix.setScale(-1f, 1f)
            ExifInterface.ORIENTATION_ROTATE_180 -> matrix.setRotate(180f)
            ExifInterface.ORIENTATION_FLIP_VERTICAL -> matrix.setScale(1f, -1f)
            ExifInterface.ORIENTATION_TRANSPOSE -> {
                matrix.setScale(-1f, 1f)
                matrix.postRotate(270f)
            }
            ExifInterface.ORIENTATION_ROTATE_90 -> matrix.setRotate(90f)
            ExifInterface.ORIENTATION_TRANSVERSE -> {
                matrix.setScale(-1f, 1f)
                matrix.postRotate(90f)
            }
            ExifInterface.ORIENTATION_ROTATE_270 -> matrix.setRotate(270f)
            else -> return bitmap
        }
        return Bitmap.createBitmap(bitmap, 0, 0, bitmap.width, bitmap.height, matrix, false)
    }
}
