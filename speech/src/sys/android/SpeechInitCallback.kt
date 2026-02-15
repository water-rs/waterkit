package waterkit.speech

class SpeechInitCallback {
    @JvmField
    var waterkit_tts_init_tx: Long = 0

    external fun onTtsInit(success: Boolean)
}
