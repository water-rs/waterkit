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
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.function.Consumer

object TranslationHelper {
    private const val STATE_REMOVED_AND_AVAILABLE = 1000
    private const val TRANSLATOR_CREATION_TIMEOUT_SECONDS = 60L

    private val executor: ScheduledExecutorService = Executors.newSingleThreadScheduledExecutor()
    private val listeners = ConcurrentHashMap<Long, Consumer<TranslationCapability>>()

    @JvmStatic
    external fun onResult(callId: Long, json: String)

    @JvmStatic
    external fun onTranslatorCreated(callId: Long, translator: Translator?)

    @JvmStatic
    external fun onTranslatorFailed(callId: Long, message: String)

    @JvmStatic
    external fun onCapabilityUpdate(listenerId: Long, json: String)

    @JvmStatic
    fun isApiSupported(): Boolean = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S

    @JvmStatic
    fun getCapabilities(context: Context, callId: Long) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
            onResult(callId, error("unavailable", "Android API 31 is required"))
            return
        }
        try {
            getCapabilitiesApi31(context, callId)
        } catch (exception: Exception) {
            onResult(callId, error("platform", describe(exception)))
        }
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private fun getCapabilitiesApi31(context: Context, callId: Long) {
        val manager = context.getSystemService(TranslationManager::class.java)
        if (manager == null) {
            onResult(callId, ok(JSONObject().put("pairs", JSONArray())))
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
                onResult(callId, ok(JSONObject().put("pairs", pairs)))
            } catch (exception: Exception) {
                onResult(callId, error("platform", describe(exception)))
            }
        }
    }

    @JvmStatic
    fun createTranslator(context: Context, source: String, target: String, callId: Long) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
            onTranslatorFailed(callId, "Android API 31 is required")
            return
        }
        try {
            createTranslatorApi31(context, source, target, callId)
        } catch (exception: Exception) {
            onTranslatorFailed(callId, describe(exception))
        }
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private fun createTranslatorApi31(context: Context, source: String, target: String, callId: Long) {
        val manager = context.getSystemService(TranslationManager::class.java)
        if (manager == null) {
            onTranslatorFailed(callId, "system translation service is unavailable for $source to $target")
            return
        }
        val delivered = AtomicBoolean(false)
        // Match frameworks/base/core/java/android/view/translation/TranslationManager.java
        // SYNC_CALLS_TIMEOUT_MS, which bounds synchronous translator creation.
        val timeout = executor.schedule(
            {
                if (delivered.compareAndSet(false, true)) {
                    onTranslatorFailed(
                        callId,
                        "the system translation service did not create a translator for $source to $target within 60 s"
                    )
                }
            },
            TRANSLATOR_CREATION_TIMEOUT_SECONDS,
            TimeUnit.SECONDS
        )
        try {
            val translationContext = TranslationContext.Builder(
                TranslationSpec(ULocale.forLanguageTag(source), TranslationSpec.DATA_FORMAT_TEXT),
                TranslationSpec(ULocale.forLanguageTag(target), TranslationSpec.DATA_FORMAT_TEXT)
            ).build()
            manager.createOnDeviceTranslator(translationContext, executor) { translator ->
                if (delivered.compareAndSet(false, true)) {
                    timeout.cancel(false)
                    if (translator == null) {
                        onTranslatorFailed(
                            callId,
                            "system translation service could not create a translator for $source to $target"
                        )
                    } else {
                        onTranslatorCreated(callId, translator)
                    }
                } else {
                    try {
                        translator?.destroy()
                    } catch (exception: Exception) {
                        Log.e("TranslationHelper", "Failed to destroy late translator", exception)
                    }
                }
            }
        } catch (exception: Exception) {
            if (delivered.compareAndSet(false, true)) {
                timeout.cancel(false)
                onTranslatorFailed(callId, describe(exception))
            }
        }
    }

    @JvmStatic
    fun translate(translator: Translator, callId: Long, textsJson: String): CancellationSignal {
        val cancellationSignal = CancellationSignal()
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
            onResult(callId, error("unavailable", "Android API 31 is required"))
            return cancellationSignal
        }
        try {
            translateApi31(translator, callId, textsJson, cancellationSignal)
        } catch (exception: Exception) {
            onResult(callId, error("platform", describe(exception)))
        }
        return cancellationSignal
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private fun translateApi31(
        translator: Translator,
        callId: Long,
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
                onResult(callId, translationResponse(response, input.length()))
            } catch (exception: Exception) {
                onResult(callId, error("platform", describe(exception)))
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

    @JvmStatic
    fun registerCapabilityUpdates(context: Context, listenerId: Long): String {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
            return error("unavailable", "Android API 31 is required")
        }
        return try {
            registerCapabilityUpdatesApi31(context, listenerId)
        } catch (exception: Exception) {
            error("platform", describe(exception))
        }
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private fun registerCapabilityUpdatesApi31(context: Context, listenerId: Long): String {
        val manager = context.getSystemService(TranslationManager::class.java)
            ?: return error("unavailable", "system translation service is unavailable")
        val listener = Consumer<TranslationCapability> { capability ->
            try {
                val status = statusForState(capability.state)
                val update = JSONObject()
                    .put("source", capability.sourceSpec.locale.toLanguageTag())
                    .put("target", capability.targetSpec.locale.toLanguageTag())
                    .put("status", status ?: JSONObject.NULL)
                onCapabilityUpdate(listenerId, ok(update))
            } catch (exception: Exception) {
                onCapabilityUpdate(
                    listenerId,
                    error("platform", describe(exception))
                )
            }
        }
        listeners[listenerId] = listener
        try {
            manager.addOnDeviceTranslationCapabilityUpdateListener(executor, listener)
        } catch (exception: Exception) {
            listeners.remove(listenerId, listener)
            throw exception
        }
        return ok(JSONObject())
    }

    @JvmStatic
    fun removeCapabilityUpdates(context: Context, listenerId: Long): String {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
            listeners.remove(listenerId)
            return error("unavailable", "Android API 31 is required")
        }
        return try {
            removeCapabilityUpdatesApi31(context, listenerId)
        } catch (exception: Exception) {
            error("platform", describe(exception))
        }
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private fun removeCapabilityUpdatesApi31(context: Context, listenerId: Long): String {
        val listener = listeners.remove(listenerId)
            ?: return ok(JSONObject())
        val manager = context.getSystemService(TranslationManager::class.java)
            ?: return error("unavailable", "system translation service is unavailable")
        manager.removeOnDeviceTranslationCapabilityUpdateListener(listener)
        return ok(JSONObject())
    }

    private fun statusForState(state: Int): String? = when (state) {
        TranslationCapability.STATE_ON_DEVICE -> "installed"
        TranslationCapability.STATE_AVAILABLE_TO_DOWNLOAD -> "needs_download"
        TranslationCapability.STATE_DOWNLOADING -> "downloading"
        TranslationCapability.STATE_NOT_AVAILABLE -> null
        // frameworks/base/core/java/android/view/translation/TranslationCapability.java
        STATE_REMOVED_AND_AVAILABLE -> "needs_download"
        else -> throw IllegalArgumentException("unknown translation capability state: $state")
    }

    private fun describe(exception: Exception) =
        exception.message ?: exception.javaClass.name

    private fun ok(value: Any): String = JSONObject().put("ok", value).toString()

    private fun error(kind: String, message: String): String {
        return JSONObject()
            .put("error", JSONObject().put("kind", kind).put("message", message))
            .toString()
    }
}
