package waterkit.speech

import android.content.Context
import android.speech.tts.TextToSpeech
import android.speech.tts.UtteranceProgressListener
import java.util.Locale

object SpeechHelper {
    private var tts: TextToSpeech? = null

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
}
