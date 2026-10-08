//! Speech synthesis via `AVFAudio` and speech recognition via the `Speech`
//! framework, called through `objc2`.

use std::sync::{Arc, Mutex};

use dispatch2::{DispatchQueue, MainThreadBound};
use futures::channel::oneshot;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, define_class, msg_send};
use objc2_avf_audio::{
    AVSpeechBoundary, AVSpeechSynthesisVoice, AVSpeechSynthesizer, AVSpeechSynthesizerDelegate,
    AVSpeechUtterance, AVSpeechUtteranceDefaultSpeechRate,
};
use objc2_foundation::{NSObjectProtocol, NSString};

use crate::{SpeechError, TtsConfig, Voice};

/// Runs `work` on the main queue: inline when the caller is already on the
/// main thread, otherwise by `exec_async` with the result carried back
/// through a oneshot. Never blocks the caller.
async fn on_main<R, F>(work: F) -> R
where
    R: Send + 'static,
    F: FnOnce(MainThreadMarker) -> R + Send + 'static,
{
    if let Some(mtm) = MainThreadMarker::new() {
        work(mtm)
    } else {
        let (tx, rx) = oneshot::channel();
        DispatchQueue::main().exec_async(move || {
            let mtm = MainThreadMarker::new().expect("exec_async runs on the main queue");
            drop(tx.send(work(mtm)));
        });
        rx.await
            .expect("the exec_async worker sends before it exits")
    }
}

/// ivars of [`SpeechUtteranceDelegate`].
pub struct SpeechUtteranceDelegateIvars {
    /// Resolved once on `didFinish`/`didCancel` (both mean success here, same
    /// as the old implementation, which only ever reported `""`).
    sender: Mutex<Option<oneshot::Sender<Result<(), SpeechError>>>>,
    /// Strong self-retain: `AVSpeechSynthesizer.delegate` is a weak property,
    /// so the delegate keeps itself alive until a terminal callback arrives.
    keep_alive: Mutex<Option<Retained<SpeechUtteranceDelegate>>>,
}

define_class!(
    /// Delegate for a single `speak` call. The request object owns its state;
    /// the `objc_setAssociatedObject` delegate hack is replaced by the
    /// `keep_alive` ivar.
    // SAFETY:
    // - The superclass NSObject does not have any subclassing requirements.
    // - `SpeechUtteranceDelegate` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[ivars = SpeechUtteranceDelegateIvars]
    pub struct SpeechUtteranceDelegate;

    unsafe impl NSObjectProtocol for SpeechUtteranceDelegate {}

    // SAFETY: `AVSpeechSynthesizerDelegate` requires `Send + Sync`; every ivar
    // is synchronized (`Mutex`), so this class is an `AnyThread` class.
    unsafe impl AVSpeechSynthesizerDelegate for SpeechUtteranceDelegate {
        #[unsafe(method(speechSynthesizer:didFinishSpeechUtterance:))]
        fn did_finish(&self, _synthesizer: &AVSpeechSynthesizer, _utterance: &AVSpeechUtterance) {
            self.resolve();
        }

        #[unsafe(method(speechSynthesizer:didCancelSpeechUtterance:))]
        fn did_cancel(&self, _synthesizer: &AVSpeechSynthesizer, _utterance: &AVSpeechUtterance) {
            self.resolve();
        }
    }
);

impl SpeechUtteranceDelegate {
    fn new(sender: oneshot::Sender<Result<(), SpeechError>>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(SpeechUtteranceDelegateIvars {
            sender: Mutex::new(Some(sender)),
            keep_alive: Mutex::new(None),
        });
        // SAFETY: `this` is a freshly allocated `SpeechUtteranceDelegate` and
        // `NSObject`'s `init` has no additional requirements.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        *this.ivars().keep_alive.lock().expect("fresh delegate") = Some(this.clone());
        this
    }

    /// Resolves the oneshot with `Ok` and releases the keep-alive so the
    /// delegate can deallocate.
    fn resolve(&self) {
        let ivars = self.ivars();
        let mut guard = ivars.sender.lock().expect("speech sender mutex poisoned");
        if let Some(sender) = guard.take() {
            let _ = sender.send(Ok(()));
        }
        drop(guard);
        *ivars.keep_alive.lock().expect("keep-alive mutex poisoned") = None;
    }
}

#[derive(Debug)]
pub struct TtsInner {
    /// The one synthesizer the engine owns, confined to the main thread
    /// (`AVSpeechSynthesizer` is not `Send`/`Sync`, and `MainThreadBound`
    /// keeps this struct `Send + Sync` like the previous zero-sized wrapper);
    /// the `Arc` lets 'static main-queue hops own a share.
    synthesizer: Arc<MainThreadBound<Retained<AVSpeechSynthesizer>>>,
}

impl TtsInner {
    pub async fn new() -> Result<Self, SpeechError> {
        // The previous implementation reported success unconditionally.
        // SAFETY: `AVSpeechSynthesizer::new` is `[[AVSpeechSynthesizer alloc]
        // init]` with no invariants; it is created on the main thread so the
        // `MainThreadBound` marker is honest.
        Ok(Self {
            synthesizer: Arc::new(
                on_main(|mtm| MainThreadBound::new(unsafe { AVSpeechSynthesizer::new() }, mtm))
                    .await,
            ),
        })
    }

    #[expect(
        clippy::unused_async,
        reason = "the async signature is part of the crate API surface and other platforms await here"
    )]
    pub async fn available_voices(&self) -> Result<Vec<Voice>, SpeechError> {
        // SAFETY: `speechVoices` is a class accessor with no invariants; each
        // voice property is a read-only accessor on a live object.
        Ok(unsafe { AVSpeechSynthesisVoice::speechVoices() }
            .iter()
            .map(|voice| Voice {
                // SAFETY: read-only accessors on a live `AVSpeechSynthesisVoice`.
                id: unsafe { voice.identifier() }.to_string(),
                name: unsafe { voice.name() }.to_string(),
                language: unsafe { voice.language() }.to_string(),
            })
            .collect())
    }

    pub async fn speak(&self, text: &str, config: &TtsConfig) -> Result<(), SpeechError> {
        // Building the utterance and installing the delegate happens on the
        // main thread so `!Send` Objective-C objects never live across an
        // `await` point.
        let synthesizer = Arc::clone(&self.synthesizer);
        let text = text.to_owned();
        let config = config.clone();
        let receiver = on_main(move |mtm| {
            let synthesizer = synthesizer.get(mtm);
            let string = NSString::from_str(&text);
            // SAFETY: `initWithString:` is the designated initializer;
            // `string` is a live `NSString`.
            let utterance =
                unsafe { AVSpeechUtterance::initWithString(AVSpeechUtterance::alloc(), &string) };
            // SAFETY: scalar property setters on a live utterance;
            // `AVSpeechUtteranceDefaultSpeechRate` is a constant extern float.
            unsafe {
                utterance.setRate(AVSpeechUtteranceDefaultSpeechRate * config.rate);
                utterance.setPitchMultiplier(config.pitch);
                utterance.setVolume(config.volume);
            }
            if let Some(voice) = &config.voice {
                let identifier = NSString::from_str(&voice.id);
                // SAFETY: `identifier` is a live `NSString`; the factory
                // returns nil for an unknown identifier, handled as `None`.
                let voice = unsafe { AVSpeechSynthesisVoice::voiceWithIdentifier(&identifier) };
                // SAFETY: property setter on a live utterance.
                unsafe { utterance.setVoice(voice.as_deref()) };
            }

            let (sender, receiver) = oneshot::channel();
            let delegate = SpeechUtteranceDelegate::new(sender);
            // SAFETY: `synthesizer` is live; `delegate` conforms to the
            // protocol and stays alive through its `keep_alive` ivar (the
            // property is weak).
            unsafe { synthesizer.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
            // SAFETY: both objects are live.
            unsafe { synthesizer.speakUtterance(&utterance) };
            receiver
        })
        .await;
        receiver
            .await
            .map_err(|_| SpeechError::Platform("speech callback dropped".into()))?
    }

    pub fn stop(&self) {
        let synthesizer = Arc::clone(&self.synthesizer);
        DispatchQueue::main().exec_async(move || {
            let mtm = MainThreadMarker::new().expect("exec_async runs on the main queue");
            // SAFETY: the synthesizer is live; the returned bool only reports
            // whether speech was stopped mid-utterance, which callers ignore.
            unsafe {
                synthesizer
                    .get(mtm)
                    .stopSpeakingAtBoundary(AVSpeechBoundary::Immediate);
            }
        });
    }

    pub async fn is_speaking(&self) -> bool {
        let synthesizer = Arc::clone(&self.synthesizer);
        // SAFETY: the synthesizer is live.
        on_main(move |mtm| unsafe { synthesizer.get(mtm).isSpeaking() }).await
    }
}

#[cfg(target_os = "ios")]
mod recognition {
    use std::ptr::NonNull;
    use std::sync::Arc;

    use block2::RcBlock;
    use dispatch2::{DispatchQueue, MainThreadBound};
    use objc2::rc::Retained;
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_avf_audio::{AVAudioEngine, AVAudioInputNode, AVAudioPCMBuffer, AVAudioTime};
    use objc2_foundation::{NSError, NSLocale, NSString};
    use objc2_speech::{
        SFSpeechAudioBufferRecognitionRequest, SFSpeechRecognitionTask, SFSpeechRecognizer,
    };

    use super::on_main;
    use crate::{RecognitionConfig, RecognitionResult, SpeechError};

    /// `SFSpeechRecognizer()?.isAvailable ?? false`, one-to-one.
    pub fn recognition_is_available() -> bool {
        // SAFETY: `new` is `[[SFSpeechRecognizer alloc] init]`, and
        // `isAvailable` is a read-only accessor.
        unsafe { SFSpeechRecognizer::new().isAvailable() }
    }

    /// The engine, input tap and task of a running session. Owned by the
    /// request object and dropped on the main thread's runloop via
    /// [`MainThreadBound`]; the raw-pointer `result_ctx` and the
    /// `std::mem::forget` on the sender are gone.
    #[derive(Debug)]
    struct SessionState {
        engine: Retained<AVAudioEngine>,
        input: Retained<AVAudioInputNode>,
        task: Retained<SFSpeechRecognitionTask>,
    }

    /// A running recognition session.
    #[derive(Debug)]
    pub struct SpeechRecognizerInner {
        session: Arc<MainThreadBound<SessionState>>,
    }

    impl SpeechRecognizerInner {
        pub async fn start(
            config: RecognitionConfig,
        ) -> Result<(Self, async_channel::Receiver<RecognitionResult>), SpeechError> {
            if !recognition_is_available() {
                return Err(SpeechError::NotAvailable);
            }
            on_main(move |mtm| Self::start_inner(mtm, &config)).await
        }

        /// All of it on the main thread: `AVAudioEngine`/`SFSpeechRecognizer`
        /// objects are `!Send` and must not live across `await`.
        fn start_inner(
            mtm: MainThreadMarker,
            config: &RecognitionConfig,
        ) -> Result<(Self, async_channel::Receiver<RecognitionResult>), SpeechError> {
            {
                let locale =
                    config
                        .language
                        .as_ref()
                        .map_or_else(NSLocale::currentLocale, |language| {
                            let identifier = NSString::from_str(language);
                            // `initWithLocaleIdentifier:` is a safe initializer;
                            // the identifier is a live `NSString`.
                            NSLocale::initWithLocaleIdentifier(NSLocale::alloc(), &identifier)
                        });
                // SAFETY: `initWithLocale:` is a designated initializer and may
                // return nil for an unsupported language.
                let recognizer = unsafe {
                    SFSpeechRecognizer::initWithLocale(SFSpeechRecognizer::alloc(), &locale)
                };
                // SAFETY: `isAvailable` is a read-only accessor on a live
                // object.
                let Some(recognizer) = recognizer.filter(|r| unsafe { r.isAvailable() }) else {
                    return Err(SpeechError::Platform(
                        "Speech recognition not available".into(),
                    ));
                };

                // SAFETY: `new` is a convenience constructor with no
                // invariants.
                let engine = unsafe { AVAudioEngine::new() };
                // SAFETY: same; `setShouldReportPartialResults` is a property
                // setter on a live request.
                let request = unsafe { SFSpeechAudioBufferRecognitionRequest::new() };
                unsafe { request.setShouldReportPartialResults(config.partial_results) };

                // SAFETY: `engine` is live; `inputNode` is the microphone
                // node.
                let input = unsafe { engine.inputNode() };
                // SAFETY: `input` is live; bus 0 is the input bus.
                let format = unsafe { input.outputFormatForBus(0) };
                let tap_request = request.clone();
                let tap_block = RcBlock::new(
                    move |buffer: NonNull<AVAudioPCMBuffer>, _when: NonNull<AVAudioTime>| {
                        // SAFETY: the buffer pointer is valid for the duration
                        // of the tap callback.
                        unsafe { tap_request.appendAudioPCMBuffer(&*buffer.as_ptr()) };
                    },
                );
                // SAFETY: `input` is live, bus 0 is the input bus, the format
                // matches the tap contract, and the node copies the block.
                unsafe {
                    input.installTapOnBus_bufferSize_format_block(
                        0,
                        1024,
                        Some(&format),
                        RcBlock::as_ptr(&tap_block).cast(),
                    );
                }

                // SAFETY: `engine` is live.
                unsafe { engine.prepare() };
                // SAFETY: `engine` is live; the `NSError` is an out-param.
                if let Err(error) = unsafe { engine.startAndReturnError() } {
                    return Err(SpeechError::Platform(
                        error.localizedDescription().to_string(),
                    ));
                }

                let (result_tx, result_rx) = async_channel::bounded(64);
                let engine_in_block = engine.clone();
                let input_in_block = input.clone();
                let result_block = RcBlock::new(
                    move |result: *mut objc2_speech::SFSpeechRecognitionResult,
                          error: *mut NSError| {
                        let mut is_final = false;
                        if !result.is_null() {
                            // SAFETY: `result` is non-null and valid for the
                            // duration of the callback.
                            let result = unsafe { &*result };
                            // SAFETY: read-only accessors on a live result; the
                            // confidence is the last segment's, or -1 like the
                            // original.
                            let (text, final_, confidence) = unsafe {
                                let transcription = result.bestTranscription();
                                let segments = transcription.segments();
                                (
                                    transcription.formattedString().to_string(),
                                    result.isFinal(),
                                    segments
                                        .lastObject()
                                        .map_or(-1.0, |segment| segment.confidence()),
                                )
                            };
                            is_final = final_;
                            let _ = result_tx.try_send(RecognitionResult {
                                text,
                                is_final,
                                confidence: if confidence >= 0.0 {
                                    Some(confidence)
                                } else {
                                    None
                                },
                            });
                        }
                        if !error.is_null() || is_final {
                            // SAFETY: the engine and input are live for as
                            // long as the task runs (the task retains this
                            // block).
                            unsafe {
                                engine_in_block.stop();
                                input_in_block.removeTapOnBus(0);
                            }
                        }
                    },
                );
                // SAFETY: `recognizer`, `request` and `result_block` are live;
                // the task retains its request while running.
                let task = unsafe {
                    recognizer.recognitionTaskWithRequest_resultHandler(&request, &result_block)
                };

                Ok((
                    Self {
                        session: Arc::new(MainThreadBound::new(
                            SessionState {
                                engine,
                                input,
                                task,
                            },
                            mtm,
                        )),
                    },
                    result_rx,
                ))
            }
        }

        pub fn stop(&self) {
            let session = Arc::clone(&self.session);
            DispatchQueue::main().exec_async(move || {
                let mtm = MainThreadMarker::new().expect("exec_async runs on the main queue");
                let state = session.get(mtm);
                // SAFETY: all three objects are live; `cancel` ends the task
                // and its callbacks.
                unsafe {
                    state.task.cancel();
                    state.engine.stop();
                    state.input.removeTapOnBus(0);
                }
            });
        }
    }
}

#[cfg(target_os = "ios")]
pub use recognition::{SpeechRecognizerInner, recognition_is_available};

/// Speech recognition exists nowhere but iOS; non-iOS Apple keeps the same
/// "not available" surface the conditional-compiled version had.
#[cfg(not(target_os = "ios"))]
#[expect(
    clippy::missing_const_for_fn,
    reason = "the iOS implementation calls into the Speech framework and is not const"
)]
pub fn recognition_is_available() -> bool {
    false
}

/// Placeholder on Apple platforms other than iOS, where
/// `recognition_is_available` already reports `false`.
#[cfg(not(target_os = "ios"))]
#[derive(Debug)]
pub struct SpeechRecognizerInner;

#[cfg(not(target_os = "ios"))]
impl SpeechRecognizerInner {
    #[expect(
        clippy::unused_async,
        reason = "the async signature is part of the crate API surface and other platforms await here"
    )]
    pub async fn start(
        _config: crate::RecognitionConfig,
    ) -> Result<(Self, async_channel::Receiver<crate::RecognitionResult>), SpeechError> {
        Err(SpeechError::NotAvailable)
    }

    #[expect(
        clippy::unused_self,
        reason = "there is no session state on non-iOS platforms"
    )]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "the iOS implementation calls into AVFAudio and is not const"
    )]
    pub fn stop(&self) {}
}
