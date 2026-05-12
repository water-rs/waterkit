use crate::{RecognitionConfig, RecognitionResult, SpeechError, TtsConfig, Voice};

#[swift_bridge::bridge]
mod ffi {
    extern "Swift" {
        fn speech_tts_init(callback: Box<dyn FnOnce(bool) -> ()>);
        fn speech_available_voices() -> String;
        fn speech_speak(
            text: &str,
            rate: f32,
            pitch: f32,
            volume: f32,
            voice_id: Option<String>,
            callback: Box<dyn FnOnce(String) -> ()>,
        );
        fn speech_stop();
        fn speech_is_speaking() -> bool;
        fn speech_recognition_available() -> bool;
        fn speech_recognition_start(
            language: Option<String>,
            partial_results: bool,
            result_ctx: u64,
            error_callback: Box<dyn FnOnce(String) -> ()>,
        );
        fn speech_recognition_stop();
    }

    extern "Rust" {
        fn on_recognition_result_raw(result_ctx: u64, text: &str, is_final: bool, confidence: f32);
    }
}

#[allow(clippy::cast_possible_truncation)]
fn on_recognition_result_raw(result_ctx: u64, text: &str, is_final: bool, confidence: f32) {
    let tx = unsafe { &*(result_ctx as usize as *const async_channel::Sender<RecognitionResult>) };
    let _ = tx.try_send(RecognitionResult {
        text: text.to_string(),
        is_final,
        confidence: if confidence >= 0.0 {
            Some(confidence)
        } else {
            None
        },
    });
}

pub fn recognition_is_available() -> bool {
    ffi::speech_recognition_available()
}

#[derive(Debug)]
pub struct TtsInner;

impl TtsInner {
    pub async fn new() -> Result<Self, SpeechError> {
        let (tx, rx) = futures::channel::oneshot::channel();
        ffi::speech_tts_init(Box::new(move |success: bool| {
            let _ = tx.send(success);
        }));
        let success = rx
            .await
            .map_err(|_| SpeechError::Platform("callback dropped".into()))?;
        if success {
            Ok(Self)
        } else {
            Err(SpeechError::NotAvailable)
        }
    }

    #[allow(clippy::unnecessary_wraps, clippy::unused_self)]
    pub fn available_voices(&self) -> Result<Vec<Voice>, SpeechError> {
        let json = ffi::speech_available_voices();
        Ok(json
            .lines()
            .filter(|l| !l.is_empty())
            .filter_map(|line| {
                let parts: Vec<&str> = line.splitn(3, '|').collect();
                if parts.len() == 3 {
                    Some(Voice {
                        id: parts[0].to_string(),
                        name: parts[1].to_string(),
                        language: parts[2].to_string(),
                    })
                } else {
                    None
                }
            })
            .collect())
    }

    pub async fn speak(&self, text: &str, config: &TtsConfig) -> Result<(), SpeechError> {
        let (tx, rx) = futures::channel::oneshot::channel();
        ffi::speech_speak(
            text,
            config.rate,
            config.pitch,
            config.volume,
            config.voice.as_ref().map(|v| v.id.clone()),
            Box::new(move |error: String| {
                if error.is_empty() {
                    let _ = tx.send(Ok(()));
                } else {
                    let _ = tx.send(Err(SpeechError::Platform(error)));
                }
            }),
        );
        rx.await
            .map_err(|_| SpeechError::Platform("callback dropped".into()))?
    }

    #[allow(clippy::unused_self)]
    pub fn stop(&self) {
        ffi::speech_stop();
    }

    #[allow(clippy::unused_self)]
    pub fn is_speaking(&self) -> bool {
        ffi::speech_is_speaking()
    }
}

#[derive(Debug)]
pub struct SpeechRecognizerInner;

impl SpeechRecognizerInner {
    pub async fn start(
        config: RecognitionConfig,
    ) -> Result<(Self, async_channel::Receiver<RecognitionResult>), SpeechError> {
        if !recognition_is_available() {
            return Err(SpeechError::NotAvailable);
        }
        let (result_tx, result_rx) = async_channel::bounded(64);
        let result_tx = Box::new(result_tx);
        let result_ctx = (&raw const *result_tx) as usize as u64;

        let (err_tx, err_rx) = futures::channel::oneshot::channel();
        ffi::speech_recognition_start(
            config.language,
            config.partial_results,
            result_ctx,
            Box::new(move |error: String| {
                let _ = err_tx.send(error);
            }),
        );
        // Check for immediate error
        let err = err_rx.await.unwrap_or_default();
        if !err.is_empty() {
            return Err(SpeechError::Platform(err));
        }
        // Leak the sender so it lives as long as needed (dropped when stop is called)
        std::mem::forget(result_tx);
        Ok((Self, result_rx))
    }

    #[allow(clippy::unused_self)]
    pub fn stop(&self) {
        ffi::speech_recognition_stop();
    }
}
