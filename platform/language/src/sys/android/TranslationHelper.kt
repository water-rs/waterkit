package waterkit.language

import android.app.PendingIntent
import android.content.Context
import android.os.Build
import android.os.CancellationSignal
import android.util.Log
import android.util.SparseArray
import android.view.translation.TranslationCapability
import android.view.translation.TranslationContext
import android.view.translation.TranslationManager
import android.view.translation.TranslationRequest
import android.view.translation.TranslationRequestValue
import android.view.translation.TranslationResponse
import android.view.translation.TranslationResponseValue
import android.view.translation.TranslationSpec
import android.view.translation.Translator
import android.icu.util.ULocale
import androidx.annotation.RequiresApi
import org.json.JSONArray
import org.json.JSONObject
import waterkit.build.NativeCallback
import waterkit.build.NativeChannel
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.function.Consumer

object TranslationHelper {
    private const val STATE_REMOVED_AND_AVAILABLE = 1000

    internal val executor: ExecutorService = Executors.newSingleThreadExecutor()

    @JvmStatic
    fun isApiSupported(): Boolean = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S

    @JvmStatic
    fun getCapabilities(context: Context, callback: NativeCallback) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
            callback.complete( error("unavailable", "Android API 31 is required"))
            return
        }
        try {
            getCapabilitiesApi31(context, callback)
        } catch (exception: Exception) {
            callback.complete( error("platform", describe(exception)))
        }
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private fun getCapabilitiesApi31(context: Context, callback: NativeCallback) {
        val manager = context.getSystemService(TranslationManager::class.java)
        if (manager == null) {
            callback.complete( ok(JSONObject().put("pairs", JSONArray())))
            return
        }
        executor.execute {
            try {
                val pairs = JSONArray()
                val capabilities = manager.getOnDeviceTranslationCapabilities(
                    TranslationSpec.DATA_FORMAT_TEXT,
                    TranslationSpec.DATA_FORMAT_TEXT
                )
                for (capability in capabilities) {
                    val state = statusForState(capability.state)
                    pairs.put(
                        JSONObject()
                            .put("source", capability.sourceSpec.locale.toLanguageTag())
                            .put("target", capability.targetSpec.locale.toLanguageTag())
                            .put("status", state ?: JSONObject.NULL)
                    )
                }
                callback.complete( ok(JSONObject().put("pairs", pairs)))
            } catch (exception: Exception) {
                callback.complete( error("platform", describe(exception)))
            }
        }
    }

    @JvmStatic
    fun createTranslator(context: Context, source: String, target: String, callback: NativeCallback) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
            callback.fail(
                    "Android API 31 is required")
            return
        }
        try {
            createTranslatorApi31(context, source, target, callback)
        } catch (exception: Exception) {
            callback.fail(
                    describe(exception))
        }
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private fun createTranslatorApi31(context: Context, source: String, target: String, callback: NativeCallback) {
        val manager = context.getSystemService(TranslationManager::class.java)
        if (manager == null) {
            callback.fail(
                    "system translation service is unavailable for $source to $target")
            return
        }
        try {
            val translationContext = TranslationContext.Builder(
                TranslationSpec(ULocale.forLanguageTag(source), TranslationSpec.DATA_FORMAT_TEXT),
                TranslationSpec(ULocale.forLanguageTag(target), TranslationSpec.DATA_FORMAT_TEXT)
            ).build()
            manager.createOnDeviceTranslator(translationContext, executor) { translator ->
                if (translator == null) {
                    callback.fail(
                        "system translation service could not create a translator for $source to $target"
                    )
                } else {
                    callback.complete(translator)
                }
            }
        } catch (exception: Exception) {
            callback.fail(describe(exception))
        }
    }

    @JvmStatic
    fun translate(translator: Translator, callback: NativeCallback, textsJson: String): CancellationSignal {
        val cancellationSignal = CancellationSignal()
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
            callback.complete( error("unavailable", "Android API 31 is required"))
            return cancellationSignal
        }
        try {
            translateApi31(translator, callback, textsJson, cancellationSignal)
        } catch (exception: Exception) {
            callback.complete( error("platform", describe(exception)))
        }
        return cancellationSignal
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private fun translateApi31(
        translator: Translator,
        callback: NativeCallback,
        textsJson: String,
        cancellationSignal: CancellationSignal
    ) {
        val input = JSONArray(textsJson)
        val values = ArrayList<TranslationRequestValue>(input.length())
        for (index in 0 until input.length()) {
            val text = input.get(index) as? String
                ?: throw IllegalArgumentException("translation input at index $index is not a string")
            values.add(TranslationRequestValue.forText(text))
        }
        val request = TranslationRequest.Builder()
            .setTranslationRequestValues(values)
            .build()
        translator.translate(request, cancellationSignal, executor) { response ->
            try {
                callback.complete( translationResponse(response, input.length()))
            } catch (exception: Exception) {
                callback.complete( error("platform", describe(exception)))
            }
        }
    }

    private fun translationResponse(response: TranslationResponse, expectedLength: Int): String {
        when (response.translationStatus) {
            TranslationResponse.TRANSLATION_STATUS_SUCCESS -> {
                val values: SparseArray<TranslationResponseValue> = response.translationResponseValues
                val output = JSONArray()
                for (index in 0 until expectedLength) {
                    val value = values.get(index)
                        ?: throw IllegalStateException("translation response is missing index $index")
                    if (value.statusCode != TranslationResponseValue.STATUS_SUCCESS) {
                        throw IllegalStateException(
                            "translation response at index $index has status ${value.statusCode}"
                        )
                    }
                    val text = value.text?.toString()
                        ?: throw IllegalStateException("translation response at index $index has null text")
                    output.put(text)
                }
                return ok(output)
            }
            TranslationResponse.TRANSLATION_STATUS_CONTEXT_UNSUPPORTED ->
                return error("unsupported_pair", "translation context is unsupported")
            else -> throw IllegalStateException(
                "translation response has status ${response.translationStatus}"
            )
        }
    }

    @JvmStatic
    fun openDownloadSettings(context: Context): String {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
            return error("unavailable", "Android API 31 is required")
        }
        return try {
            openDownloadSettingsApi31(context)
        } catch (exception: Exception) {
            error("platform", describe(exception))
        }
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private fun openDownloadSettingsApi31(context: Context): String {
        val manager = context.getSystemService(TranslationManager::class.java)
            ?: return error("unavailable", "system translation service is unavailable")
        val intent: PendingIntent = manager.onDeviceTranslationSettingsActivityIntent
            ?: return error("unavailable", "translation settings activity is unavailable")
        intent.send()
        return ok(JSONObject())
    }

    /**
     * One capability-update registration, owned by the Rust handle as a global
     * reference; [CapabilityUpdates.close] removes the platform listener and
     * ends the stream. Returns null when the device has no system translation
     * service (below API 31, or no `TranslationManager`); throws when
     * registration itself fails.
     */
    @JvmStatic
    fun registerCapabilityUpdates(context: Context, channel: NativeChannel): CapabilityUpdates? {
        if (!isApiSupported()) {
            return null
        }
        val manager = context.getSystemService(TranslationManager::class.java) ?: return null
        return CapabilityUpdates(manager, channel).also { it.start() }
    }

    internal fun statusForState(state: Int): String? = when (state) {
        TranslationCapability.STATE_ON_DEVICE -> "installed"
        TranslationCapability.STATE_AVAILABLE_TO_DOWNLOAD -> "needs_download"
        TranslationCapability.STATE_DOWNLOADING -> "downloading"
        TranslationCapability.STATE_NOT_AVAILABLE -> null
        // frameworks/base/core/java/android/view/translation/TranslationCapability.java
        STATE_REMOVED_AND_AVAILABLE -> "needs_download"
        else -> throw IllegalArgumentException("unknown translation capability state: $state")
    }

    internal fun describe(exception: Exception) =
        exception.message ?: exception.javaClass.name

    internal fun ok(value: Any): String = JSONObject().put("ok", value).toString()

    internal fun error(kind: String, message: String): String {
        return JSONObject()
            .put("error", JSONObject().put("kind", kind).put("message", message))
            .toString()
    }
}

/**
 * One capability-update registration. Lives on the Rust
 * `CapabilityUpdates` handle as a global reference; [close] removes the
 * platform listener and ends the [channel] stream.
 */
@RequiresApi(Build.VERSION_CODES.S)
class CapabilityUpdates internal constructor(
    private val manager: TranslationManager,
    private val channel: NativeChannel,
) {
    private val listener = Consumer<TranslationCapability> { capability ->
        try {
            val status = TranslationHelper.statusForState(capability.state)
            val update = JSONObject()
                .put("source", capability.sourceSpec.locale.toLanguageTag())
                .put("target", capability.targetSpec.locale.toLanguageTag())
                .put("status", status ?: JSONObject.NULL)
            channel.send(TranslationHelper.ok(update))
        } catch (exception: Exception) {
            channel.send(
                TranslationHelper.error("platform", TranslationHelper.describe(exception))
            )
        }
    }

    /** Installs the platform listener. Throws when registration fails. */
    internal fun start() {
        manager.addOnDeviceTranslationCapabilityUpdateListener(
            TranslationHelper.executor,
            listener,
        )
    }

    /** Removes the listener and ends the stream. Called again is a no-op. */
    fun close() {
        manager.removeOnDeviceTranslationCapabilityUpdateListener(listener)
        channel.close()
    }
}
