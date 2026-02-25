# waterkit-speech

Cross-platform text-to-speech and speech recognition for Rust.

Part of the [Waterkit](https://github.com/water-rs/waterkit) ecosystem.

## Features

- **Text-to-Speech**: Speak text aloud with configurable voice, rate, pitch, and volume
- **Speech Recognition**: Real-time speech-to-text with partial and final results
- Voice enumeration and selection

## Platform Support

| Platform | Status |
|----------|--------|
| iOS      | Native (AVSpeechSynthesizer / SFSpeechRecognizer via Swift bridge) |
| macOS    | Native (NSSpeechSynthesizer / SFSpeechRecognizer via Swift bridge) |
| Android  | Native (TextToSpeech / SpeechRecognizer via JNI/Kotlin) |
| Windows  | Native TTS; speech recognition pending |
| Linux    | Native TTS (espeak); speech recognition pending |

## Usage

```rust
use waterkit_speech::{Tts, TtsConfig, SpeechRecognizer, RecognitionConfig};

async fn tts_example() -> Result<(), waterkit_speech::SpeechError> {
    let tts = Tts::new().await?;

    // List available voices
    let voices = tts.available_voices()?;

    // Speak with default config
    tts.speak("Hello from Rust!", &TtsConfig::default()).await?;

    Ok(())
}

// Speech recognition is currently available on iOS, macOS, and Android.
async fn recognition_example() -> Result<(), waterkit_speech::SpeechError> {
    let (recognizer, rx) = SpeechRecognizer::start(RecognitionConfig::default()).await?;

    // Receive recognition results
    while let Ok(result) = rx.recv().await {
        if result.is_final {
            // Use result.text
            break;
        }
    }

    recognizer.stop().await;
    Ok(())
}
```

## License

MIT OR Apache-2.0
