package waterkit.vision

import android.content.Context
import com.google.android.gms.common.api.OptionalModuleApi
import com.google.android.gms.common.moduleinstall.ModuleInstall
import com.google.android.gms.common.moduleinstall.ModuleInstallRequest
import com.google.android.gms.tasks.Tasks
import com.google.mlkit.vision.common.InputImage
import com.google.mlkit.vision.text.TextRecognition
import com.google.mlkit.vision.text.TextRecognizer
import com.google.mlkit.vision.text.TextRecognizerOptionsInterface
import com.google.mlkit.vision.text.chinese.ChineseTextRecognizerOptions
import com.google.mlkit.vision.text.devanagari.DevanagariTextRecognizerOptions
import com.google.mlkit.vision.text.japanese.JapaneseTextRecognizerOptions
import com.google.mlkit.vision.text.korean.KoreanTextRecognizerOptions
import com.google.mlkit.vision.text.latin.TextRecognizerOptions
import java.util.concurrent.TimeUnit

/**
 * Kotlin half of `waterkit-vision`'s `text` feature on Android: thin
 * wrappers over the unbundled ML Kit script recognizers. Each recognizer
 * engine lives in a Play services module `prepareModule` installs on
 * demand.
 *
 * Every method that touches a Play services `Task` blocks on `Tasks.await`
 * and must run on the dedicated threads the Rust side spawns for it.
 */
object VisionTextHelper {
    // Module codes mirrored in `sys/android/mlkit.rs`: what `prepareModule`
    // and `recognizeText` take.
    private const val MODULE_LATIN = 0
    private const val MODULE_CHINESE = 1
    private const val MODULE_DEVANAGARI = 2
    private const val MODULE_JAPANESE = 3
    private const val MODULE_KOREAN = 4

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

    // One recognizer per script; the engines themselves live in Play
    // services' dynamite modules.
    private val recognizers = HashMap<Int, TextRecognizer>()

    /** The `OptionalModuleApi` identifying each module's installable unit. */
    private fun moduleApi(script: Int): OptionalModuleApi = when (script) {
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
        MODULE_KOREAN -> TextRecognition.getClient(
            KoreanTextRecognizerOptions.Builder().build(),
        )
        else -> throw IllegalArgumentException("unknown text module $script")
    }

    /**
     * Downloads `script`'s recognizer module when it is not installed,
     * returning once it is. Any failure throws, which the Rust side reports
     * as model-unavailable.
     */
    @JvmStatic
    fun prepareModule(context: Context, module: Int) {
        val client = ModuleInstall.getClient(context)
        val api = moduleApi(module)
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

    private fun recognizerFor(script: Int): TextRecognizer =
        synchronized(recognizers) {
            recognizers.getOrPut(script) {
                val options: TextRecognizerOptionsInterface = when (script) {
                    MODULE_CHINESE -> ChineseTextRecognizerOptions.Builder().build()
                    MODULE_DEVANAGARI -> DevanagariTextRecognizerOptions.Builder().build()
                    MODULE_JAPANESE -> JapaneseTextRecognizerOptions.Builder().build()
                    MODULE_KOREAN -> KoreanTextRecognizerOptions.Builder().build()
                    MODULE_LATIN -> TextRecognizerOptions.Builder().build()
                    else -> throw IllegalArgumentException("unknown text recognizer $script")
                }
                TextRecognition.getClient(options)
            }
        }

    /** Every line of text in `input`, with its elements, in the image's stored orientation. */
    @JvmStatic
    fun recognizeText(input: InputImage, script: Int): Array<TextRow> {
        val text = Tasks.await(
            recognizerFor(script).process(input),
            MlKitInput.DETECTION_TIMEOUT_MS,
            TimeUnit.MILLISECONDS,
        )
        return text.textBlocks.flatMap { block -> block.lines }.map { line ->
            TextRow(
                line.text,
                line.confidence,
                MlKitInput.corners(line.cornerPoints, line.boundingBox),
                line.elements.map { it.text }.toTypedArray(),
                FloatArray(line.elements.size) { line.elements[it].confidence },
                line.elements.map {
                    MlKitInput.corners(it.cornerPoints, it.boundingBox)
                }.toTypedArray(),
            )
        }.toTypedArray()
    }
}
