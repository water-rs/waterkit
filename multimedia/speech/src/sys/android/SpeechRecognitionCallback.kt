package waterkit.speech

class SpeechRecognitionCallback {
    @JvmField
    var waterkit_recognition_tx: Long = 0

    @JvmField
    var waterkit_recognition_init_tx: Long = 0

    external fun onRecognitionReady()
    external fun onRecognitionResult(text: String, isFinal: Boolean, confidence: Float)
    external fun onRecognitionError(errorCode: Int, message: String)
}
