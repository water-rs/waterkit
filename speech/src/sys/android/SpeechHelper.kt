package waterkit.speech

import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.speech.RecognitionListener
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer
import android.speech.tts.TextToSpeech
import java.util.Locale

object SpeechHelper {
    private var tts: TextToSpeech? = null
    private var recognizer: SpeechRecognizer? = null
    private var recognitionSessionId: Long = 0L

    @JvmStatic
    external fun onRecognitionResult(
        sessionId: Long,
        text: String,
        isFinal: Boolean,
        confidence: Float
    )

    @JvmStatic
    external fun onRecognitionError(sessionId: Long, code: Int)

    @JvmStatic
    fun initTts(context: Context, callback: SpeechInitCallback) {
        tts = TextToSpeech(context) { status ->
            callback.onTtsInit(status == TextToSpeech.SUCCESS)
        }
    }

    @JvmStatic
    fun speak(text: String, rate: Float, pitch: Float, volume: Float, languageTag: String?) {
        val engine = tts ?: return
        engine.setSpeechRate(rate)
        engine.setPitch(pitch)
        if (languageTag != null) {
            engine.setLanguage(Locale.forLanguageTag(languageTag))
        }
        val params = android.os.Bundle()
        params.putFloat(TextToSpeech.Engine.KEY_PARAM_VOLUME, volume)
        engine.speak(text, TextToSpeech.QUEUE_FLUSH, params, "waterkit_utterance")
    }

    @JvmStatic
    fun stop() {
        tts?.stop()
    }

    @JvmStatic
    fun isSpeaking(): Boolean {
        return tts?.isSpeaking ?: false
    }

    @JvmStatic
    fun getAvailableVoices(): Array<String> {
        val engine = tts ?: return emptyArray()
        return engine.voices?.map { voice ->
            "${voice.name}|${voice.name}|${voice.locale.toLanguageTag()}"
        }?.toTypedArray() ?: emptyArray()
    }

    @JvmStatic
    fun shutdown() {
        tts?.shutdown()
        tts = null
    }

    @JvmStatic
    fun isRecognitionAvailable(context: Context): Boolean {
        return SpeechRecognizer.isRecognitionAvailable(context)
    }

    @JvmStatic
    fun startRecognition(
        context: Context,
        languageTag: String?,
        partialResults: Boolean,
        sessionId: Long
    ): Boolean {
        if (!SpeechRecognizer.isRecognitionAvailable(context)) {
            return false
        }

        stopRecognition()
        recognitionSessionId = sessionId

        val speechRecognizer = SpeechRecognizer.createSpeechRecognizer(context.applicationContext)
        speechRecognizer.setRecognitionListener(object : RecognitionListener {
            override fun onReadyForSpeech(params: Bundle?) {}

            override fun onBeginningOfSpeech() {}

            override fun onRmsChanged(rmsdB: Float) {}

            override fun onBufferReceived(buffer: ByteArray?) {}

            override fun onEndOfSpeech() {}

            override fun onError(error: Int) {
                if (recognitionSessionId != 0L) {
                    onRecognitionError(recognitionSessionId, error)
                }
            }

            override fun onResults(results: Bundle?) {
                val sessionId = recognitionSessionId
                if (sessionId == 0L) {
                    return
                }
                val matches = results
                    ?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)
                    ?: return
                val text = matches.firstOrNull() ?: return
                val confidence = results
                    .getFloatArray(SpeechRecognizer.CONFIDENCE_SCORES)
                    ?.firstOrNull()
                    ?: -1.0f
                onRecognitionResult(sessionId, text, true, confidence)
            }

            override fun onPartialResults(partialResultsBundle: Bundle?) {
                val sessionId = recognitionSessionId
                if (sessionId == 0L) {
                    return
                }
                val matches = partialResultsBundle
                    ?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)
                    ?: return
                val text = matches.firstOrNull() ?: return
                onRecognitionResult(sessionId, text, false, -1.0f)
            }

            override fun onEvent(eventType: Int, params: Bundle?) {}
        })

        val intent = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
            putExtra(
                RecognizerIntent.EXTRA_LANGUAGE_MODEL,
                RecognizerIntent.LANGUAGE_MODEL_FREE_FORM
            )
            putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, partialResults)
            if (!languageTag.isNullOrBlank()) {
                putExtra(RecognizerIntent.EXTRA_LANGUAGE, languageTag)
            }
        }

        speechRecognizer.startListening(intent)
        recognizer = speechRecognizer
        return true
    }

    @JvmStatic
    fun stopRecognition() {
        recognizer?.stopListening()
        recognizer?.cancel()
        recognizer?.destroy()
        recognizer = null
        recognitionSessionId = 0L
    }
}
