import Foundation
import AVFoundation
#if os(iOS)
import Speech
#endif

private var synthesizer = AVSpeechSynthesizer()

func speech_tts_init(callback: __private__RustFnOnceCallbackBoolNoRet) {
    callback.call(true)
}

func speech_available_voices() -> RustString {
    let voices = AVSpeechSynthesisVoice.speechVoices()
    let result = voices.map { voice in
        "\(voice.identifier)|\(voice.name)|\(voice.language)"
    }.joined(separator: "\n")
    return RustString(result)
}

func speech_speak(
    text: RustStr,
    rate: Float,
    pitch: Float,
    volume: Float,
    voice_id: Optional<RustString>,
    callback: __private__RustFnOnceCallbackStringNoRet
) {
    let textStr = text.toString()
    let utterance = AVSpeechUtterance(string: textStr)
    utterance.rate = AVSpeechUtteranceDefaultSpeechRate * rate
    utterance.pitchMultiplier = pitch
    utterance.volume = volume

    if let voiceId = voice_id?.toString() {
        utterance.voice = AVSpeechSynthesisVoice(identifier: voiceId)
    }

    let delegate = SpeechDelegate(callback: callback)
    synthesizer.delegate = delegate
    // Keep delegate alive
    objc_setAssociatedObject(synthesizer, "delegate", delegate, .OBJC_ASSOCIATION_RETAIN)
    synthesizer.speak(utterance)
}

func speech_stop() {
    synthesizer.stopSpeaking(at: .immediate)
}

func speech_is_speaking() -> Bool {
    return synthesizer.isSpeaking
}

func speech_recognition_available() -> Bool {
    #if os(iOS)
    return SFSpeechRecognizer()?.isAvailable ?? false
    #else
    return false
    #endif
}

func speech_recognition_start(
    language: Optional<RustString>,
    partial_results: Bool,
    result_ctx: UnsafeMutableRawPointer,
    error_callback: __private__RustFnOnceCallbackStringNoRet
) {
    #if os(iOS)
    let locale: Locale
    if let lang = language?.toString() {
        locale = Locale(identifier: lang)
    } else {
        locale = Locale.current
    }
    guard let recognizer = SFSpeechRecognizer(locale: locale), recognizer.isAvailable else {
        error_callback.call("Speech recognition not available")
        return
    }

    let audioEngine = AVAudioEngine()
    let request = SFSpeechAudioBufferRecognitionRequest()
    request.shouldReportPartialResults = partial_results

    let inputNode = audioEngine.inputNode
    let recordingFormat = inputNode.outputFormat(forBus: 0)
    inputNode.installTap(onBus: 0, bufferSize: 1024, format: recordingFormat) { buffer, _ in
        request.append(buffer)
    }

    audioEngine.prepare()
    do {
        try audioEngine.start()
    } catch {
        error_callback.call(error.localizedDescription)
        return
    }

    recognizer.recognitionTask(with: request) { result, error in
        if let result = result {
            let text = result.bestTranscription.formattedString
            let isFinal = result.isFinal
            let confidence = result.bestTranscription.segments.last?.confidence ?? -1.0
            on_recognition_result_raw(result_ctx, text, isFinal, confidence)
        }
        if error != nil || (result?.isFinal ?? false) {
            audioEngine.stop()
            inputNode.removeTap(onBus: 0)
        }
    }

    error_callback.call("")
    #else
    error_callback.call("Speech recognition not available on macOS")
    #endif
}

func speech_recognition_stop() {
    // Recognition task will be invalidated and cleaned up
}

class SpeechDelegate: NSObject, AVSpeechSynthesizerDelegate {
    let callback: __private__RustFnOnceCallbackStringNoRet

    init(callback: __private__RustFnOnceCallbackStringNoRet) {
        self.callback = callback
    }

    func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didFinish utterance: AVSpeechUtterance) {
        callback.call("")
    }

    func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didCancel utterance: AVSpeechUtterance) {
        callback.call("")
    }
}
