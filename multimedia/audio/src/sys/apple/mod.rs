//! Apple platform (iOS/macOS) media session and native audio player,
//! implemented directly on `MediaPlayer` / `AVFoundation` through `objc2`.
//!
//! `AVAudioSession` and `AVAudioPlayer` are absent from `objc2-av-foundation`
//! 0.3.x (the header translator emits empty files for them) and from `objc2`'s
//! main branch, so the small surfaces this crate needs are declared here.

use crate::{MediaCommand, MediaError, MediaMetadata, PlaybackState, PlaybackStatus};
#[cfg(target_os = "ios")]
use objc2::runtime::ProtocolObject;
use objc2::{rc::Retained, runtime::AnyObject};
use objc2_foundation::{NSMutableDictionary, NSNumber, NSString};
use objc2_media_player::{
    MPChangePlaybackPositionCommandEvent, MPMediaItemArtwork, MPMediaItemPropertyAlbumTitle,
    MPMediaItemPropertyArtist, MPMediaItemPropertyArtwork, MPMediaItemPropertyPlaybackDuration,
    MPMediaItemPropertyTitle, MPNowPlayingInfoCenter, MPNowPlayingInfoPropertyElapsedPlaybackTime,
    MPNowPlayingInfoPropertyPlaybackRate, MPNowPlayingPlaybackState, MPRemoteCommandCenter,
    MPRemoteCommandEvent, MPRemoteCommandHandlerStatus, MPSkipIntervalCommandEvent,
};
use std::{
    collections::VecDeque,
    ptr::NonNull,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

#[cfg(feature = "apple-artwork")]
mod host_bridge;

/// Run `work` on the main queue — inline when the caller is already on the
/// main thread, otherwise through `exec_async` resolved by a oneshot.
///
/// Private copy of `waterkit_core::apple::on_main` (water-rs/waterkit#299);
/// the signature matches so the swap is an import change only.
#[cfg(all(target_os = "ios", feature = "playback"))]
pub async fn on_main<R, F>(work: F) -> R
where
    F: FnOnce(objc2::MainThreadMarker) -> R + Send + 'static,
    R: Send + 'static,
{
    if let Some(mtm) = objc2::MainThreadMarker::new() {
        work(mtm)
    } else {
        let (sender, receiver) = futures::channel::oneshot::channel();
        dispatch2::DispatchQueue::main().exec_async(move || {
            let mtm = objc2::MainThreadMarker::new()
                .expect("exec_async closure must run on the main queue");
            let _ = sender.send(work(mtm));
        });
        receiver
            .await
            .expect("main-queue completion was dropped before replying")
    }
}

// ---------------------------------------------------------------------------
// Command queue (NSCondition-backed, mirroring the previous channel).
// ---------------------------------------------------------------------------

#[derive(Default)]
struct CommandQueue {
    state: Mutex<CommandQueueState>,
    ready: Condvar,
}

#[derive(Default)]
struct CommandQueueState {
    commands: VecDeque<MediaCommand>,
    closed: bool,
}

impl CommandQueue {
    fn send(&self, command: MediaCommand) {
        {
            let mut state = self.state.lock().unwrap();
            if state.closed {
                return;
            }
            state.commands.push_back(command);
        }
        self.ready.notify_one();
    }

    /// Blocks until a command is available or the queue is closed. Queued
    /// commands are still delivered after `close`, matching the previous
    /// channel's drain-on-close behavior.
    fn receive(&self) -> Option<MediaCommand> {
        let mut state = self.state.lock().unwrap();
        while state.commands.is_empty() && !state.closed {
            state = self.ready.wait(state).unwrap();
        }
        state.commands.pop_front()
    }

    fn close(&self) {
        self.state.lock().unwrap().closed = true;
        self.ready.notify_all();
    }
}

// ---------------------------------------------------------------------------
// Per-session state
//
// `MPRemoteCommandCenter` is a single process-global object, so the
// previous implementation kept a registry that routed remote commands to the most
// recently activated session. Here each session owns a `SessionCore` and
// `activate` removes every registered handler (`-[MPRemoteCommand
// removeTarget:]` with nil removes all targets) before installing this
// session's blocks, which yields the same "last activated wins" behavior
// without a global registry. One divergence: closing the active session
// leaves the command center unowned until another session performs a call
// and re-activates; the previous implementation promoted the previous
// session immediately.
// ---------------------------------------------------------------------------

/// `NSNotificationCenter` observer tokens for audio-session events (iOS).
/// They are main-thread objects, so they live behind `MainThreadBound` and
/// are only touched through `get_on_main`.
#[cfg(target_os = "ios")]
type ObserverTokens = dispatch2::MainThreadBound<
    Mutex<Vec<Retained<ProtocolObject<dyn objc2_foundation::NSObjectProtocol>>>>,
>;

struct SessionCore {
    active: AtomicBool,
    commands: CommandQueue,
    #[cfg(target_os = "ios")]
    observer_tokens: ObserverTokens,
    /// Keeps the macOS silent-activation player and its delegate alive for
    /// the duration of the session.
    #[cfg(target_os = "macos")]
    _silent_delegate: Retained<macos::WaterkitSilentPlayerDelegate>,
}

impl SessionCore {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            active: AtomicBool::new(false),
            commands: CommandQueue::default(),
            #[cfg(target_os = "ios")]
            observer_tokens: dispatch2::run_on_main(|mtm| {
                dispatch2::MainThreadBound::new(Mutex::new(Vec::new()), mtm)
            }),

            #[cfg(target_os = "macos")]
            _silent_delegate: macos::activate_audio_session_with_silence(),
        })
    }

    /// Make this session the recipient of remote-control commands and
    /// system audio-session events.
    fn activate(self: &Arc<Self>) {
        self.install_command_handlers();
        self.active.store(true, Ordering::Release);
    }

    fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    fn deactivate(&self) -> bool {
        self.active.swap(false, Ordering::AcqRel)
    }

    fn install_command_handlers(self: &Arc<Self>) {
        // SAFETY: `sharedCommandCenter` returns the process-wide remote
        // command center and each accessor is a documented getter.
        let center = unsafe { MPRemoteCommandCenter::sharedCommandCenter() };

        // SAFETY: `play` through `previous` below take no event payload.
        unsafe {
            self.install(&center.playCommand(), |_| Some(MediaCommand::Play));
            self.install(&center.pauseCommand(), |_| Some(MediaCommand::Pause));
            self.install(&center.togglePlayPauseCommand(), |_| {
                Some(MediaCommand::PlayPause)
            });
            self.install(&center.stopCommand(), |_| Some(MediaCommand::Stop));
            self.install(&center.nextTrackCommand(), |_| Some(MediaCommand::Next));
            self.install(&center.previousTrackCommand(), |_| {
                Some(MediaCommand::Previous)
            });
            self.install(&center.changePlaybackPositionCommand(), |event| {
                event
                    .as_ref()
                    .downcast_ref::<MPChangePlaybackPositionCommandEvent>()
                    .map(|position_event| {
                        MediaCommand::Seek(Duration::from_secs_f64(position_event.positionTime()))
                    })
            });
        }

        // SAFETY: `skipForwardCommand`/`skipBackwardCommand` return the
        // process-wide skip commands; `[15]` mirrors the previous
        // `preferredIntervals = [15]`.
        let intervals =
            objc2_foundation::NSArray::from_retained_slice(&[NSNumber::numberWithDouble(15.0)]);
        let skip_forward = unsafe { center.skipForwardCommand() };
        unsafe { skip_forward.setPreferredIntervals(&intervals) };
        unsafe {
            self.install(&skip_forward, |event| {
                event
                    .as_ref()
                    .downcast_ref::<MPSkipIntervalCommandEvent>()
                    .map(|skip_event| {
                        MediaCommand::SeekForward(Duration::from_secs_f64(skip_event.interval()))
                    })
            });
        }
        let skip_backward = unsafe { center.skipBackwardCommand() };
        unsafe { skip_backward.setPreferredIntervals(&intervals) };
        unsafe {
            self.install(&skip_backward, |event| {
                event
                    .as_ref()
                    .downcast_ref::<MPSkipIntervalCommandEvent>()
                    .map(|skip_event| {
                        MediaCommand::SeekBackward(Duration::from_secs_f64(skip_event.interval()))
                    })
            });
        }
    }

    /// Replace every handler on `command` with one that forwards to this
    /// session's queue. `command` is `&MPRemoteCommand`;
    /// `MPSkipIntervalCommand` derefs to it. The command retains the
    /// returned target object for as long as the handler is installed, so
    /// no token storage is needed here — `removeTarget(nil)` drops it.
    ///
    /// SAFETY: callers only pass commands obtained from
    /// `MPRemoteCommandCenter::sharedCommandCenter()`.
    unsafe fn install(
        self: &Arc<Self>,
        command: &objc2_media_player::MPRemoteCommand,
        send: impl Fn(NonNull<MPRemoteCommandEvent>) -> Option<MediaCommand> + Send + 'static,
    ) {
        // SAFETY: `removeTarget(nil)` detaches every handler — including a
        // previously activated session's — so exactly one session receives
        // remote commands at a time.
        unsafe {
            command.removeTarget(None);
            command.setEnabled(true);
        }
        let core = Arc::clone(self);
        let block = block2::RcBlock::new(
            move |event: NonNull<MPRemoteCommandEvent>| -> MPRemoteCommandHandlerStatus {
                if let Some(command) = send(event)
                    && core.is_active()
                {
                    core.commands.send(command);
                }
                MPRemoteCommandHandlerStatus::Success
            },
        );
        // SAFETY: the block signature matches
        // `addTargetWithHandler:`'s `MPRemoteCommandHandler` type; the block
        // captures an `Arc` keeping the session state alive for the
        // target's lifetime.
        // The returned target token is retained by the command itself for
        // as long as the handler is registered; `removeTarget(nil)` in the
        // next activation releases it, so it need not be stored.
        unsafe {
            command.addTargetWithHandler(&*block2::RcBlock::as_ptr(&block).cast());
        }
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// The process-wide now-playing info center.
fn now_playing_center() -> Retained<MPNowPlayingInfoCenter> {
    // SAFETY: `defaultCenter` returns the shared singleton.
    unsafe { MPNowPlayingInfoCenter::defaultCenter() }
}

/// A mutable copy of the current now-playing dictionary, or an empty one.
fn current_info() -> Retained<NSMutableDictionary<NSString, AnyObject>> {
    let merged = NSMutableDictionary::<NSString, AnyObject>::new();
    // SAFETY: `nowPlayingInfo` returns the center's dictionary when set.
    if let Some(existing) = unsafe { now_playing_center().nowPlayingInfo() } {
        merged.setDictionary(&existing);
    }
    merged
}

// ---------------------------------------------------------------------------
// Public Rust surface (unchanged signatures)
// ---------------------------------------------------------------------------

pub struct MediaSessionInner {
    core: Arc<SessionCore>,
    command_receiver: async_channel::Receiver<MediaCommand>,
    command_worker: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for MediaSessionInner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MediaSessionInner")
            .field("session_id", &(Arc::as_ptr(&self.core) as usize))
            .finish_non_exhaustive()
    }
}

impl MediaSessionInner {
    #[expect(
        clippy::unnecessary_wraps,
        reason = "the public Result-returning signature is fixed by the crate API"
    )]
    pub fn new() -> Result<Self, MediaError> {
        let core = SessionCore::new();
        core.activate();
        #[cfg(target_os = "ios")]
        ios::register_system_event_observers(&core);

        let (command_sender, command_receiver) = async_channel::unbounded();
        let worker_core = Arc::clone(&core);
        let command_worker = std::thread::spawn(move || {
            while let Some(command) = worker_core.commands.receive() {
                if command_sender.send_blocking(command).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            core,
            command_receiver,
            command_worker: Some(command_worker),
        })
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "the public Result-returning signature is fixed by the crate API"
    )]
    pub fn set_metadata(&self, metadata: &MediaMetadata) -> Result<(), MediaError> {
        self.core.activate();

        let info = NSMutableDictionary::<NSString, AnyObject>::new();
        let title = metadata.title().unwrap_or_default();
        if !title.is_empty() {
            info.insert(
                unsafe { MPMediaItemPropertyTitle },
                &NSString::from_str(title),
            );
        }
        let artist = metadata.artist().unwrap_or_default();
        if !artist.is_empty() {
            info.insert(
                unsafe { MPMediaItemPropertyArtist },
                &NSString::from_str(artist),
            );
        }
        let album = metadata.album().unwrap_or_default();
        if !album.is_empty() {
            info.insert(
                unsafe { MPMediaItemPropertyAlbumTitle },
                &NSString::from_str(album),
            );
        }
        if let Some(duration) = metadata.duration() {
            info.insert(
                unsafe { MPMediaItemPropertyPlaybackDuration },
                &NSNumber::numberWithDouble(duration.as_secs_f64()),
            );
        }
        if let Some(artwork) = metadata.artwork() {
            info.insert(
                unsafe { MPMediaItemPropertyArtwork },
                &*platform_artwork(artwork),
            );
        }

        // SAFETY: `info` contains only `NSString`, `NSNumber` and
        // `MPMediaItemArtwork` values under the documented keys.
        unsafe { now_playing_center().setNowPlayingInfo(Some(&info)) };
        Ok(())
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "the public Result-returning signature is fixed by the crate API"
    )]
    pub fn set_playback_state(&self, state: &PlaybackState) -> Result<(), MediaError> {
        self.core.activate();

        let info = current_info();
        if let Some(position) = state.position() {
            info.insert(
                unsafe { MPNowPlayingInfoPropertyElapsedPlaybackTime },
                &NSNumber::numberWithDouble(position.as_secs_f64()),
            );
        }
        info.insert(
            unsafe { MPNowPlayingInfoPropertyPlaybackRate },
            &NSNumber::numberWithDouble(state.rate()),
        );

        // SAFETY: process-wide command center accessors.
        let commands = unsafe { MPRemoteCommandCenter::sharedCommandCenter() };
        unsafe {
            commands
                .nextTrackCommand()
                .setEnabled(state.queue_navigation_controls().next_enabled());
            commands
                .previousTrackCommand()
                .setEnabled(state.queue_navigation_controls().previous_enabled());
        }

        let center = now_playing_center();
        // SAFETY: `info` contains only `NSString`/`NSNumber` values.
        unsafe { center.setNowPlayingInfo(Some(&info)) };

        match state.status() {
            PlaybackStatus::Stopped => unsafe {
                center.setPlaybackState(MPNowPlayingPlaybackState::Stopped);
            },
            PlaybackStatus::Paused => unsafe {
                center.setPlaybackState(MPNowPlayingPlaybackState::Paused);
            },
            PlaybackStatus::Playing => unsafe {
                center.setPlaybackState(MPNowPlayingPlaybackState::Playing);
            },
        }
        Ok(())
    }

    #[cfg_attr(
        target_os = "macos",
        expect(
            clippy::unnecessary_wraps,
            reason = "iOS returns a real Result; macOS has no audio-focus concept"
        )
    )]
    pub fn request_audio_focus(&self) -> Result<(), MediaError> {
        self.core.activate();
        #[cfg(target_os = "ios")]
        {
            ios::request_audio_focus()
        }
        #[cfg(target_os = "macos")]
        {
            // macOS has no audio-focus concept; activation is a no-op.
            Ok(())
        }
    }

    #[cfg_attr(
        target_os = "macos",
        expect(
            clippy::unnecessary_wraps,
            reason = "iOS returns a real Result; macOS has no audio-focus concept"
        )
    )]
    pub fn abandon_audio_focus(&self) -> Result<(), MediaError> {
        self.core.activate();
        #[cfg(target_os = "ios")]
        {
            ios::abandon_audio_focus()
        }
        #[cfg(target_os = "macos")]
        {
            Ok(())
        }
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "the public Result-returning signature is fixed by the crate API"
    )]
    pub fn clear(&self) -> Result<(), MediaError> {
        if self.core.deactivate() {
            // SAFETY: nil clears the center's now-playing information.
            unsafe { now_playing_center().setNowPlayingInfo(None) };
        }
        #[cfg(target_os = "ios")]
        ios::unregister_system_event_observers(&self.core);
        self.core.commands.close();
        Ok(())
    }

    pub fn command_receiver(&self) -> async_channel::Receiver<MediaCommand> {
        self.command_receiver.clone()
    }
}

impl Drop for MediaSessionInner {
    fn drop(&mut self) {
        if let Err(error) = self.clear() {
            tracing::error!(%error, "failed to clear Apple media session during shutdown");
        }
        if let Some(worker) = self.command_worker.take() {
            worker
                .join()
                .expect("Apple media command worker must not panic during shutdown");
        }
    }
}

/// Platform image used by `MPMediaItemArtwork`.
#[cfg(target_os = "ios")]
fn platform_artwork(artwork: &crate::MediaArtwork) -> Retained<MPMediaItemArtwork> {
    use objc2::AnyThread;
    use objc2_foundation::NSData;
    use objc2_ui_kit::UIImage;

    let data = NSData::from_vec(artwork.encoded().to_vec());
    let image = UIImage::imageWithData(&data)
        .expect("Apple media session received invalid encoded artwork");
    let size = unsafe { image.size() };
    let block =
        block2::RcBlock::new(move |_size: objc2_core_foundation::CGSize| NonNull::from(&*image));
    // `initWithBoundsSize:requestHandler:` is only generated for macOS in
    // `objc2-media-player` 0.3.x (the iOS variant returns `UIImage`), so it
    // is sent manually here.
    // SAFETY: `initWithBoundsSize:requestHandler:` is a documented
    // `MPMediaItemArtwork` initializer on iOS; the block signature matches
    // `(CGSize) -> UIImage *`.
    unsafe {
        objc2::msg_send![
            MPMediaItemArtwork::alloc(),
            initWithBoundsSize: size,
            requestHandler: block2::RcBlock::as_ptr(&block)
                .cast::<block2::DynBlock<dyn Fn(objc2_core_foundation::CGSize) -> NonNull<UIImage>>>()
        ]
    }
}

#[cfg(target_os = "macos")]
fn platform_artwork(artwork: &crate::MediaArtwork) -> Retained<MPMediaItemArtwork> {
    use objc2::AnyThread;
    use objc2_app_kit::NSImage;
    use objc2_foundation::NSData;

    let data = NSData::from_vec(artwork.encoded().to_vec());
    let image = NSImage::initWithData(NSImage::alloc(), &data)
        .expect("Apple media session received invalid encoded artwork");
    let size = image.size();
    let block =
        block2::RcBlock::new(move |_size: objc2_core_foundation::CGSize| NonNull::from(&*image));
    // SAFETY: the block signature matches the generated
    // `initWithBoundsSize:requestHandler:` `request_handler` parameter.
    unsafe {
        MPMediaItemArtwork::initWithBoundsSize_requestHandler(
            MPMediaItemArtwork::alloc(),
            size,
            &*block2::RcBlock::as_ptr(&block)
                .cast::<block2::DynBlock<dyn Fn(objc2_core_foundation::CGSize) -> NonNull<NSImage>>>(),
        )
    }
}

// ---------------------------------------------------------------------------
// iOS: AVAudioSession (absent from objc2-av-foundation 0.3.x — declared here)
// ---------------------------------------------------------------------------

#[cfg(target_os = "ios")]
mod ios {
    use super::{MediaCommand, MediaError, SessionCore};
    use block2::RcBlock;
    use objc2::{
        extern_class, extern_conformance, extern_methods,
        rc::Retained,
        runtime::{NSObject, NSObjectProtocol, ProtocolObject},
    };
    use objc2_foundation::{
        NSError, NSNotification, NSNotificationCenter, NSNotificationName, NSNumber,
        NSOperationQueue, NSString,
    };
    use std::ptr::NonNull;
    use std::sync::Arc;

    extern_class!(
        /// [Apple's documentation](https://developer.apple.com/documentation/avfaudio/avaudiosession?language=objc)
        #[unsafe(super(NSObject))]
        #[name = "AVAudioSession"]
        #[derive(Debug, PartialEq, Eq, Hash)]
        pub struct AVAudioSession;
    );

    extern_conformance!(
        unsafe impl NSObjectProtocol for AVAudioSession {}
    );

    #[allow(non_upper_case_globals)]
    unsafe extern "C" {
        /// `AVAudioSessionCategoryPlayback` — uninterrupted playback.
        pub static AVAudioSessionCategoryPlayback: &'static NSString;
        /// `AVAudioSessionModeDefault` — default audio mode.
        pub static AVAudioSessionModeDefault: &'static NSString;

        /// Posted when an audio-session interruption begins/ends.
        pub static AVAudioSessionInterruptionNotification: &'static NSNotificationName;
        /// Posted when the audio route changes.
        pub static AVAudioSessionRouteChangeNotification: &'static NSNotificationName;
        /// Posted when the system asks to silence secondary audio.
        pub static AVAudioSessionSilenceSecondaryAudioHintNotification: &'static NSNotificationName;

        /// `userInfo` key carrying `AVAudioSessionInterruptionType`.
        pub static AVAudioSessionInterruptionTypeKey: &'static NSString;
        /// `userInfo` key carrying `AVAudioSessionInterruptionOptions`.
        pub static AVAudioSessionInterruptionOptionKey: &'static NSString;
        /// `userInfo` key carrying `AVAudioSessionRouteChangeReason`.
        pub static AVAudioSessionRouteChangeReasonKey: &'static NSString;
        /// `userInfo` key carrying `AVAudioSessionSilenceSecondaryAudioHintType`.
        pub static AVAudioSessionSilenceSecondaryAudioHintTypeKey: &'static NSString;
    }

    // `AVAudioSessionInterruptionType` (AVAudioSessionTypes.h).
    const INTERRUPTION_TYPE_ENDED: usize = 0;
    const INTERRUPTION_TYPE_BEGAN: usize = 1;
    // `AVAudioSessionInterruptionOptionShouldResume`.
    const INTERRUPTION_OPTION_SHOULD_RESUME: usize = 1;
    // `AVAudioSessionRouteChangeReasonOldDeviceUnavailable`.
    const ROUTE_CHANGE_OLD_DEVICE_UNAVAILABLE: usize = 2;
    // `AVAudioSessionSilenceSecondaryAudioHintType`.
    const SILENCE_HINT_END: usize = 0;
    const SILENCE_HINT_BEGIN: usize = 1;
    // `AVAudioSessionSetActiveOptionNotifyOthersOnDeactivation`.
    const SET_ACTIVE_NOTIFY_OTHERS_ON_DEACTIVATION: usize = 1;

    impl AVAudioSession {
        extern_methods!(
            /// The process-wide audio session.
            #[unsafe(method(sharedInstance))]
            #[unsafe(method_family = none)]
            pub unsafe fn shared_instance() -> Retained<AVAudioSession>;

            /// `setCategory:mode:error:`
            #[unsafe(method(setCategory:mode:error:_))]
            #[unsafe(method_family = none)]
            pub unsafe fn set_category_mode_error(
                &self,
                category: &NSString,
                mode: &NSString,
            ) -> Result<(), Retained<NSError>>;

            /// `setActive:error:`
            #[unsafe(method(setActive:error:_))]
            #[unsafe(method_family = none)]
            pub unsafe fn set_active_error(&self, active: bool) -> Result<(), Retained<NSError>>;

            /// `setActive:withOptions:error:`
            #[unsafe(method(setActive:withOptions:error:_))]
            #[unsafe(method_family = none)]
            pub unsafe fn set_active_with_options_error(
                &self,
                active: bool,
                options: usize,
            ) -> Result<(), Retained<NSError>>;
        );
    }

    fn send_if_active(core: &Arc<SessionCore>, command: MediaCommand) {
        if core.is_active() {
            core.commands.send(command);
        }
    }

    /// Read `userInfo[key]` as an unsigned integer.
    fn user_info_number(notification: &NSNotification, key: &'static NSString) -> Option<usize> {
        let info = notification.userInfo()?;
        info.objectForKey(key)?
            .downcast_ref::<NSNumber>()
            .map(objc2_foundation::NSNumber::unsignedIntegerValue)
    }

    fn handle_interruption(core: &Arc<SessionCore>, notification: &NSNotification) {
        match user_info_number(notification, unsafe { AVAudioSessionInterruptionTypeKey }) {
            Some(INTERRUPTION_TYPE_BEGAN) => {
                send_if_active(core, MediaCommand::AudioFocusLostTransient);
            }
            Some(INTERRUPTION_TYPE_ENDED) => {
                let options =
                    user_info_number(notification, unsafe { AVAudioSessionInterruptionOptionKey })
                        .unwrap_or(0);
                if options & INTERRUPTION_OPTION_SHOULD_RESUME != 0 {
                    send_if_active(core, MediaCommand::AudioFocusGained);
                } else {
                    send_if_active(core, MediaCommand::AudioFocusLost);
                }
            }
            Some(other) => panic!("unsupported AVAudioSession interruption type {other}"),
            None => {}
        }
    }

    fn handle_route_change(core: &Arc<SessionCore>, notification: &NSNotification) {
        match user_info_number(notification, unsafe { AVAudioSessionRouteChangeReasonKey }) {
            Some(ROUTE_CHANGE_OLD_DEVICE_UNAVAILABLE) => {
                send_if_active(core, MediaCommand::AudioBecomingNoisy);
            }
            // newDeviceAvailable(1), categoryChange(3), override(4),
            // wakeFromSleep(5), noSuitableRouteForCategory(6),
            // routeConfigurationChange(7), unknown(0) — ignored as before.
            Some(0..=7) | None => {}
            Some(other) => panic!("unsupported AVAudioSession route change reason {other}"),
        }
    }

    fn handle_silence_hint(core: &Arc<SessionCore>, notification: &NSNotification) {
        match user_info_number(notification, unsafe {
            AVAudioSessionSilenceSecondaryAudioHintTypeKey
        }) {
            Some(SILENCE_HINT_BEGIN) => {
                send_if_active(core, MediaCommand::AudioFocusLostDuck);
            }
            Some(SILENCE_HINT_END) => {
                send_if_active(core, MediaCommand::AudioFocusGained);
            }
            Some(other) => {
                panic!("unsupported AVAudioSession silence secondary audio hint type {other}")
            }
            None => {}
        }
    }

    fn observe(
        center: &NSNotificationCenter,
        session: &AVAudioSession,
        name: &'static NSNotificationName,
        core: &Arc<SessionCore>,
        handler: fn(&Arc<SessionCore>, &NSNotification),
    ) -> Retained<ProtocolObject<dyn NSObjectProtocol>> {
        let core = Arc::clone(core);
        let block = RcBlock::new(move |notification: NonNull<NSNotification>| {
            // SAFETY: the notification pointer is valid for the block call.
            handler(&core, unsafe { notification.as_ref() });
        });
        // SAFETY: `name` is a documented notification; the block signature
        // matches `addObserverForName:object:queue:usingBlock:`.
        unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(name),
                Some(session),
                Some(&NSOperationQueue::mainQueue()),
                &*RcBlock::as_ptr(&block).cast(),
            )
        }
    }

    pub(super) fn register_system_event_observers(core: &Arc<SessionCore>) {
        core.observer_tokens.get_on_main(|tokens| {
            // SAFETY: `sharedInstance`/`defaultCenter` return process
            // singletons; both are created here on the main queue and stay
            // local to this call.
            let session = unsafe { AVAudioSession::shared_instance() };
            let center = NSNotificationCenter::defaultCenter();
            tokens.lock().unwrap().extend([
                observe(
                    &center,
                    &session,
                    unsafe { AVAudioSessionInterruptionNotification },
                    core,
                    handle_interruption,
                ),
                observe(
                    &center,
                    &session,
                    unsafe { AVAudioSessionRouteChangeNotification },
                    core,
                    handle_route_change,
                ),
                observe(
                    &center,
                    &session,
                    unsafe { AVAudioSessionSilenceSecondaryAudioHintNotification },
                    core,
                    handle_silence_hint,
                ),
            ]);
        });
    }

    pub(super) fn unregister_system_event_observers(core: &Arc<SessionCore>) {
        core.observer_tokens.get_on_main(|tokens| {
            let mut tokens = tokens.lock().unwrap();
            if tokens.is_empty() {
                return;
            }
            // SAFETY: every token came from `addObserverForName:...` above.
            let center = NSNotificationCenter::defaultCenter();
            for token in tokens.drain(..) {
                unsafe { center.removeObserver((*token).as_ref()) };
            }
        });
    }

    pub(super) fn request_audio_focus() -> Result<(), MediaError> {
        // SAFETY: `sharedInstance` returns the process-wide session; the
        // category/mode constants are exported by AVFAudio.
        let session = unsafe { AVAudioSession::shared_instance() };
        let configured = unsafe {
            session
                .set_category_mode_error(AVAudioSessionCategoryPlayback, AVAudioSessionModeDefault)
        };
        configured
            .and_then(|()| unsafe { session.set_active_error(true) })
            .map_err(|_error| MediaError::AudioFocusDenied)
    }

    pub(super) fn abandon_audio_focus() -> Result<(), MediaError> {
        // SAFETY: `sharedInstance` returns the process-wide session.
        let session = unsafe { AVAudioSession::shared_instance() };
        unsafe {
            session.set_active_with_options_error(false, SET_ACTIVE_NOTIFY_OTHERS_ON_DEACTIVATION)
        }
        .map_err(|_error| MediaError::UpdateFailed("failed to deactivate AVAudioSession".into()))
    }
}

// ---------------------------------------------------------------------------
// macOS: AVAudioPlayer silent-activation pulse (absent from
// objc2-av-foundation 0.3.x — declared here)
// ---------------------------------------------------------------------------

#[expect(
    clippy::missing_safety_doc,
    reason = "extern_protocol! items are linted at the macro call site; the optional delegate callbacks carry no implementer-side safety contract"
)]
#[cfg(target_os = "macos")]
mod macos {
    use objc2::{
        AnyThread, DeclaredClass, define_class, extern_class, extern_conformance, extern_methods,
        extern_protocol, msg_send,
        rc::Retained,
        runtime::{NSObject, NSObjectProtocol, ProtocolObject},
    };
    use objc2_foundation::{NSData, NSError};
    use std::sync::Mutex;

    extern_class!(
        /// [Apple's documentation](https://developer.apple.com/documentation/avfaudio/avaudioplayer?language=objc)
        #[unsafe(super(NSObject))]
        #[name = "AVAudioPlayer"]
        #[derive(Debug, PartialEq, Eq, Hash)]
        pub struct AVAudioPlayer;
    );

    extern_conformance!(
        unsafe impl NSObjectProtocol for AVAudioPlayer {}
    );

    /// `AVAudioPlayer` is safe to call and share across threads; generated
    /// objc2-* crates emit the same impls for thread-safe classes.
    ///
    /// # Safety
    ///
    /// Apple documents `AVAudioPlayer` as thread-safe; the silent-pulse
    /// player here is additionally only touched through a `Mutex`.
    #[expect(
        clippy::non_send_fields_in_send_ty,
        reason = "extern class: the opaque object pointer is the only field"
    )]
    unsafe impl Send for AVAudioPlayer {}
    /// # Safety
    ///
    /// See the `Send` impl.
    unsafe impl Sync for AVAudioPlayer {}

    impl AVAudioPlayer {
        extern_methods!(
            #[unsafe(method(setVolume:))]
            #[unsafe(method_family = none)]
            pub unsafe fn set_volume(&self, volume: f32);

            #[unsafe(method(setDelegate:))]
            #[unsafe(method_family = none)]
            pub unsafe fn set_delegate(
                &self,
                delegate: Option<&ProtocolObject<dyn AVAudioPlayerDelegate>>,
            );

            #[unsafe(method(prepareToPlay))]
            #[unsafe(method_family = none)]
            pub unsafe fn prepare_to_play(&self) -> bool;

            #[unsafe(method(play))]
            #[unsafe(method_family = none)]
            pub unsafe fn play(&self) -> bool;
        );
    }

    extern_protocol!(
        /// [Apple's documentation](https://developer.apple.com/documentation/avfaudio/avaudioplayerdelegate?language=objc)
        pub unsafe trait AVAudioPlayerDelegate: NSObjectProtocol {
            /// `audioPlayerDidFinishPlaying:successfully:`
            #[optional]
            #[unsafe(method(audioPlayerDidFinishPlaying:successfully:))]
            #[unsafe(method_family = none)]
            unsafe fn audio_player_did_finish_playing_successfully(
                &self,
                player: &AVAudioPlayer,
                flag: bool,
            );

            /// `audioPlayerDecodeErrorDidOccur:error:`
            #[optional]
            #[unsafe(method(audioPlayerDecodeErrorDidOccur:error:))]
            #[unsafe(method_family = none)]
            unsafe fn audio_player_decode_error_did_occur_error(
                &self,
                player: &AVAudioPlayer,
                error: Option<&NSError>,
            );
        }
    );

    pub struct SilentPlayerIvars {
        player: Mutex<Option<Retained<AVAudioPlayer>>>,
    }

    define_class!(
        /// Delegate that releases the silent pulse player once playback ends,
        /// mirroring the previous silent-player delegate.
        #[unsafe(super(NSObject))]
        #[thread_kind = AnyThread]
        #[name = "WaterkitSilentPlayerDelegate"]
        #[ivars = SilentPlayerIvars]
        pub struct WaterkitSilentPlayerDelegate;

        unsafe impl NSObjectProtocol for WaterkitSilentPlayerDelegate {}

        unsafe impl AVAudioPlayerDelegate for WaterkitSilentPlayerDelegate {
            #[unsafe(method(audioPlayerDidFinishPlaying:successfully:))]
            fn audio_player_did_finish_playing_successfully(
                &self,
                _player: &AVAudioPlayer,
                _flag: bool,
            ) {
                self.ivars().player.lock().unwrap().take();
            }

            #[unsafe(method(audioPlayerDecodeErrorDidOccur:error:))]
            fn audio_player_decode_error_did_occur_error(
                &self,
                _player: &AVAudioPlayer,
                error: Option<&NSError>,
            ) {
                self.ivars().player.lock().unwrap().take();
                if let Some(error) = error {
                    tracing::warn!(%error, "silent audio activation decode failed");
                }
            }
        }
    );

    impl WaterkitSilentPlayerDelegate {
        fn new() -> Retained<Self> {
            let this = Self::alloc().set_ivars(SilentPlayerIvars {
                player: Mutex::new(None),
            });
            // SAFETY: `init` on `NSObject` is safe to call on `Allocated`.
            unsafe { msg_send![super(this), init] }
        }
    }

    /// A 0.1 s stereo 16-bit PCM WAV of silence — the payload previously
    /// built for `activateAudioSessionWithSilence`.
    fn silent_wav() -> Vec<u8> {
        const SAMPLE_RATE: u32 = 44100;
        const CHANNELS: u16 = 2;
        const BITS_PER_SAMPLE: u16 = 16;
        let num_samples = (SAMPLE_RATE as usize) / 10;
        let data_size =
            u32::try_from(num_samples * usize::from(CHANNELS) * usize::from(BITS_PER_SAMPLE / 8))
                .expect("silent WAV payload fits in a WAV data chunk");
        let mut wav = Vec::with_capacity(44 + data_size as usize);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_size).to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&CHANNELS.to_le_bytes());
        wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
        wav.extend_from_slice(
            &(SAMPLE_RATE * u32::from(CHANNELS) * u32::from(BITS_PER_SAMPLE / 8)).to_le_bytes(),
        );
        wav.extend_from_slice(&(CHANNELS * (BITS_PER_SAMPLE / 8)).to_le_bytes());
        wav.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_size.to_le_bytes());
        wav.resize(44 + data_size as usize, 0);
        wav
    }

    /// Emit a short silent pulse so `MPNowPlayingInfoCenter` registers in
    /// Control Center. Returns the delegate keeping the player alive.
    pub(super) fn activate_audio_session_with_silence() -> Retained<WaterkitSilentPlayerDelegate> {
        let delegate = WaterkitSilentPlayerDelegate::new();
        let data = NSData::from_vec(silent_wav());
        // SAFETY: `initWithData:error:` is a documented `AVAudioPlayer`
        // initializer; `data` is a complete WAV payload.
        // `initWithData:error:` is sent manually because the `_error`
        // convention does not cover `init`-family methods in
        // `objc2::extern_methods!`.
        // SAFETY: `initWithData:error:` is a documented `AVAudioPlayer`
        // initializer; `data` is a complete WAV payload, and `error` is a
        // valid `NSError **` out-pointer.
        let player: Option<Retained<AVAudioPlayer>> = {
            let mut error: Option<Retained<NSError>> = None;
            let result = unsafe {
                msg_send![
                    AVAudioPlayer::alloc(),
                    initWithData: &*data,
                    error: &mut error
                ]
            };
            if let Some(error) = error {
                tracing::warn!(%error, "failed to create silent audio player");
            }
            result
        };
        let Some(player) = player else {
            return delegate;
        };
        unsafe {
            player.set_volume(0.0);
            player.set_delegate(Some(ProtocolObject::from_ref(&*delegate)));
            player.prepare_to_play();
            if !player.play() {
                tracing::warn!("silent audio activation pulse did not start playback");
                return delegate;
            }
        }
        *delegate.ivars().player.lock().unwrap() = Some(player);
        delegate
    }
}

// ---------------------------------------------------------------------------
// iOS native audio player (AVPlayer) — compiled only with `playback`
// ---------------------------------------------------------------------------

#[cfg(all(target_os = "ios", feature = "playback"))]
mod player {
    use super::ios::{AVAudioSession, AVAudioSessionCategoryPlayback, AVAudioSessionModeDefault};
    use crate::{PlaybackStatus, PlayerError};
    use dispatch2::MainThreadBound;
    use objc2::rc::Retained;
    use objc2_av_foundation::{
        AVAudioTimePitchAlgorithmSpectral, AVAudioTimePitchAlgorithmVarispeed, AVPlayer,
        AVPlayerItem,
    };
    use objc2_core_media::CMTime;
    use objc2_foundation::{NSNumber, NSString, NSURL};
    use objc2_media_player::{
        MPMediaItemPropertyPlaybackDuration, MPNowPlayingInfoPropertyElapsedPlaybackTime,
        MPNowPlayingInfoPropertyPlaybackRate, MPNowPlayingPlaybackState,
    };
    use std::{
        sync::{
            Mutex,
            atomic::{AtomicBool, AtomicU32, Ordering},
        },
        time::Duration,
    };

    const MINIMUM_PLAYBACK_RATE: f32 = 0.25;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct NativeAudioPlayerState {
        pub status: PlaybackStatus,
        pub position: Option<Duration>,
        pub duration: Option<Duration>,
    }

    /// `AVPlayer` is not `Send` in `objc2-av-foundation` 0.3.x, and its
    /// factory methods take a `MainThreadMarker`, so the stored player lives
    /// behind `MainThreadBound` and every access runs on the main queue
    /// (inline when the caller is already on it).
    #[derive(Debug)]
    pub struct NativeAudioPlayerInner {
        player: MainThreadBound<Mutex<Option<Retained<AVPlayer>>>>,
        requested_rate: AtomicU32,
        preserve_pitch: AtomicBool,
        volume: AtomicU32,
    }

    /// Pick the pitch algorithm for the preserve-pitch flag; the constants
    /// are availability-gated `Option`s in `objc2-av-foundation`.
    fn pitch_algorithm(
        preserve_pitch: bool,
    ) -> &'static objc2_av_foundation::AVAudioTimePitchAlgorithm {
        // SAFETY: the pitch-algorithm constants are extern statics.
        unsafe {
            if preserve_pitch {
                AVAudioTimePitchAlgorithmSpectral
            } else {
                AVAudioTimePitchAlgorithmVarispeed
            }
        }
        .expect("AVAudioTimePitchAlgorithm constants must be available")
    }

    impl NativeAudioPlayerInner {
        pub fn new() -> Result<Self, PlayerError> {
            // SAFETY: `sharedInstance` returns the process-wide session.
            let session = unsafe { AVAudioSession::shared_instance() };
            let configured = unsafe {
                session.set_category_mode_error(
                    AVAudioSessionCategoryPlayback,
                    AVAudioSessionModeDefault,
                )
            };
            // `AVAudioSession` category/mode constants are extern statics;
            // reading them is covered by the enclosing `unsafe` block.
            configured
                .and_then(|()| unsafe { session.set_active_error(true) })
                .map_err(|error| {
                    tracing::error!(%error, "failed to activate AVAudioSession");
                    PlayerError::LoadFailed("Apple audio player failed to load media".into())
                })?;
            Ok(Self {
                player: dispatch2::run_on_main(|mtm| MainThreadBound::new(Mutex::new(None), mtm)),
                requested_rate: AtomicU32::new(1.0f32.to_bits()),
                preserve_pitch: AtomicBool::new(true),
                volume: AtomicU32::new(1.0f32.to_bits()),
            })
        }

        fn requested_rate(&self) -> f32 {
            f32::from_bits(self.requested_rate.load(Ordering::Relaxed))
        }

        fn volume(&self) -> f32 {
            f32::from_bits(self.volume.load(Ordering::Relaxed))
        }

        fn current_state(&self) -> NativeAudioPlayerState {
            self.player.get_on_main(|player| {
                let Some(player) = player.lock().unwrap().clone() else {
                    return NativeAudioPlayerState {
                        status: PlaybackStatus::Stopped,
                        position: None,
                        duration: None,
                    };
                };
                // SAFETY: `rate`, `currentTime`, `currentItem` and
                // `duration` are documented getters.
                let status = if unsafe { player.rate() } > 0.0 {
                    PlaybackStatus::Playing
                } else {
                    PlaybackStatus::Paused
                };
                let position = unsafe { player.currentTime().seconds() };
                let duration = unsafe { player.currentItem() }
                    .map_or(-1.0, |item| unsafe { item.duration().seconds() });
                NativeAudioPlayerState {
                    status,
                    position: (position.is_finite() && position >= 0.0)
                        .then(|| Duration::from_secs_f64(position)),
                    duration: (duration.is_finite() && duration >= 0.0)
                        .then(|| Duration::from_secs_f64(duration)),
                }
            })
        }

        fn update_now_playing(&self) {
            let has_player = self
                .player
                .get_on_main(|player| player.lock().unwrap().is_some());
            let center = super::now_playing_center();
            if !has_player {
                // SAFETY: nil clears now-playing info.
                unsafe { center.setNowPlayingInfo(None) };
                return;
            }
            let info = super::current_info();
            let state = self.current_state();
            if let Some(position) = state.position {
                info.insert(
                    unsafe { MPNowPlayingInfoPropertyElapsedPlaybackTime },
                    &NSNumber::numberWithDouble(position.as_secs_f64()),
                );
            }
            if let Some(duration) = state.duration {
                info.insert(
                    unsafe { MPMediaItemPropertyPlaybackDuration },
                    &NSNumber::numberWithDouble(duration.as_secs_f64()),
                );
            }
            let (rate, playback_state) = match state.status {
                PlaybackStatus::Playing => (
                    f64::from(self.requested_rate()),
                    MPNowPlayingPlaybackState::Playing,
                ),
                PlaybackStatus::Paused => (0.0, MPNowPlayingPlaybackState::Paused),
                _ => (0.0, MPNowPlayingPlaybackState::Stopped),
            };
            info.insert(
                unsafe { MPNowPlayingInfoPropertyPlaybackRate },
                &NSNumber::numberWithDouble(rate),
            );
            // SAFETY: `info` contains only `NSNumber` values under
            // documented keys.
            unsafe {
                center.setNowPlayingInfo(Some(&info));
                center.setPlaybackState(playback_state);
            }
        }

        fn load_url_object(&self, url: &NSURL) {
            dispatch2::run_on_main(|mtm| {
                let guard = self.player.get(mtm);
                let mut player_slot = guard.lock().unwrap();
                // Stop the current player first (`stopCurrentPlayer`).
                if let Some(player) = player_slot.as_ref() {
                    unsafe {
                        player.pause();
                        player.replaceCurrentItemWithPlayerItem(None);
                    }
                }
                *player_slot = None;

                // SAFETY: `playerItemWithURL:` is the documented item
                // factory; it is `MainThreadOnly`, and `mtm` proves we are
                // on the main queue.
                let item = unsafe { AVPlayerItem::playerItemWithURL(url, mtm) };
                // SAFETY: `audioTimePitchAlgorithm` accepts the documented
                // algorithm constants.
                unsafe {
                    item.setAudioTimePitchAlgorithm(pitch_algorithm(
                        self.preserve_pitch.load(Ordering::Relaxed),
                    ));
                };

                // SAFETY: `playerWithPlayerItem:` builds a player for
                // `item`; also `MainThreadOnly`.
                let player = unsafe { AVPlayer::playerWithPlayerItem(Some(&item), mtm) };
                unsafe {
                    player.setVolume(self.volume());
                    player.setAutomaticallyWaitsToMinimizeStalling(true);
                    player.pause();
                }
                *player_slot = Some(player);
            });
            self.update_now_playing();
        }

        pub fn load_file(&self, path: &str) {
            self.load_url_object(&NSURL::fileURLWithPath(&NSString::from_str(path)));
        }

        pub fn load_url(&self, url: &str) -> Result<(), PlayerError> {
            let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) else {
                tracing::error!("audio_player_load_url received invalid URL");
                return Err(PlayerError::LoadFailed(
                    "Apple audio player failed to load media".into(),
                ));
            };
            self.load_url_object(&url);
            Ok(())
        }

        /// Run `work` with the loaded `AVPlayer` on the main queue, or fail
        /// with `PlaybackFailed` when nothing is loaded.
        fn with_player<R: Send>(
            &self,
            work: impl FnOnce(&AVPlayer) -> R + Send,
        ) -> Result<R, PlayerError> {
            self.player.get_on_main(|player| {
                let Some(player) = player.lock().unwrap().clone() else {
                    return Err(PlayerError::PlaybackFailed(
                        "Apple audio player operation failed".into(),
                    ));
                };
                Ok(work(&player))
            })
        }

        pub fn play(&self) -> Result<(), PlayerError> {
            self.with_player(|player| unsafe {
                // SAFETY: `play`/`setRate` are documented `AVPlayer` methods.
                player.play();
                player.setRate(self.requested_rate().max(MINIMUM_PLAYBACK_RATE));
            })?;
            self.update_now_playing();
            Ok(())
        }

        pub fn pause(&self) -> Result<(), PlayerError> {
            self.with_player(|player| unsafe {
                // SAFETY: `pause` is a documented `AVPlayer` method.
                player.pause();
            })?;
            self.update_now_playing();
            Ok(())
        }

        pub fn stop(&self) {
            self.player.get_on_main(|player| {
                let mut guard = player.lock().unwrap();
                if let Some(player) = guard.as_ref() {
                    unsafe {
                        player.pause();
                        player.replaceCurrentItemWithPlayerItem(None);
                    }
                }
                *guard = None;
            });
            // SAFETY: nil clears now-playing info.
            unsafe { super::now_playing_center().setNowPlayingInfo(None) };
        }

        pub fn seek(&self, position: Duration) -> Result<(), PlayerError> {
            self.with_player(|player| unsafe {
                // SAFETY: `seekToTime:` accepts any `CMTime`.
                player.seekToTime(CMTime::with_seconds(position.as_secs_f64(), 1000));
            })?;
            self.update_now_playing();
            Ok(())
        }

        pub fn set_volume(&self, volume: f32) {
            self.volume.store(volume.to_bits(), Ordering::Relaxed);
            self.player.get_on_main(|player| {
                let loaded = player.lock().unwrap().clone();
                if let Some(player) = loaded {
                    // SAFETY: `setVolume` is a documented `AVPlayer` setter.
                    unsafe { player.setVolume(volume) };
                }
            });
        }

        pub fn set_playback_rate(&self, rate: f32) {
            self.requested_rate.store(rate.to_bits(), Ordering::Relaxed);
            self.player.get_on_main(|player| {
                let loaded = player.lock().unwrap().clone();
                if let Some(player) = loaded {
                    // SAFETY: `rate`/`setRate` are documented accessors.
                    if unsafe { player.rate() } > 0.0 {
                        unsafe { player.setRate(rate.max(MINIMUM_PLAYBACK_RATE)) };
                    }
                }
            });
            self.update_now_playing();
        }

        pub fn set_preserve_pitch(&self, preserve_pitch: bool) {
            self.preserve_pitch.store(preserve_pitch, Ordering::Relaxed);
            self.player.get_on_main(|player| {
                let loaded = player.lock().unwrap().clone();
                if let Some(item) = loaded.and_then(|player| unsafe {
                    // SAFETY: `currentItem` is a documented getter.
                    player.currentItem()
                }) {
                    // SAFETY: `setAudioTimePitchAlgorithm` accepts the
                    // documented algorithm constants.
                    unsafe { item.setAudioTimePitchAlgorithm(pitch_algorithm(preserve_pitch)) };
                }
            });
            self.update_now_playing();
        }

        pub fn state(&self) -> NativeAudioPlayerState {
            self.current_state()
        }
    }
}

#[cfg(all(target_os = "ios", feature = "playback"))]
pub use player::{NativeAudioPlayerInner, NativeAudioPlayerState};
