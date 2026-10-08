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

/// Shared main-queue hop; re-exported so `crate::sys::on_main` keeps
/// working for the iOS player paths.
#[cfg(all(feature = "playback", target_os = "ios"))]
pub use waterkit_core::apple::on_main;

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
// session's blocks, which yields the same "last activated wins" behavior.
// `SESSIONS` keeps weak references in activation order so `clear` can
// promote the previous session, exactly like the earlier registry did.
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
    /// Set by `register_system_event_observers` once the main-queue
    /// registration has run.
    #[cfg(target_os = "ios")]
    observer_tokens: std::sync::OnceLock<ObserverTokens>,
    /// Keeps the macOS silent-activation player and its delegate alive for
    /// the duration of the session.
    #[cfg(target_os = "macos")]
    _silent_delegate: Retained<macos::WaterkitSilentPlayerDelegate>,
}

/// Live session cores in activation order; `clear` promotes the previous
/// session to own the command center when the active one closes.
static SESSIONS: Mutex<Vec<std::sync::Weak<SessionCore>>> = Mutex::new(Vec::new());

impl SessionCore {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            active: AtomicBool::new(false),
            commands: CommandQueue::default(),
            #[cfg(target_os = "ios")]
            observer_tokens: std::sync::OnceLock::new(),

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
        SESSIONS.lock().unwrap().push(Arc::downgrade(&core));
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
        let was_active = self.core.deactivate();
        if was_active {
            // SAFETY: nil clears the center's now-playing information.
            unsafe { now_playing_center().setNowPlayingInfo(None) };
        }
        let mut sessions = SESSIONS.lock().unwrap();
        sessions.retain(|weak| {
            weak.upgrade()
                .is_some_and(|core| !Arc::ptr_eq(&core, &self.core))
        });
        if was_active {
            // Promote the previous session so the command center keeps an
            // owner, as the earlier registry did.
            while let Some(weak) = sessions.last() {
                if let Some(previous) = weak.upgrade() {
                    previous.activate();
                    break;
                }
                sessions.pop();
            }
        }
        drop(sessions);
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
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_avf_audio::{
        AVAudioSession, AVAudioSessionCategoryOptions, AVAudioSessionCategoryPlayback,
        AVAudioSessionInterruptionNotification, AVAudioSessionInterruptionOptionKey,
        AVAudioSessionInterruptionOptions, AVAudioSessionInterruptionType,
        AVAudioSessionInterruptionTypeKey, AVAudioSessionModeDefault,
        AVAudioSessionRouteChangeNotification, AVAudioSessionRouteChangeReason,
        AVAudioSessionRouteChangeReasonKey, AVAudioSessionSetActiveOptions,
        AVAudioSessionSilenceSecondaryAudioHintNotification,
        AVAudioSessionSilenceSecondaryAudioHintType,
        AVAudioSessionSilenceSecondaryAudioHintTypeKey,
    };
    use objc2_foundation::{
        NSNotification, NSNotificationCenter, NSNotificationName, NSNumber, NSOperationQueue,
        NSString,
    };
    use std::ptr::NonNull;
    use std::sync::Arc;

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
        let Some(kind) = user_info_number(notification, unsafe {
            AVAudioSessionInterruptionTypeKey.expect("AVAudioSessionInterruptionTypeKey is present")
        })
        .map(AVAudioSessionInterruptionType) else {
            return;
        };
        match kind {
            AVAudioSessionInterruptionType::Began => {
                send_if_active(core, MediaCommand::AudioFocusLostTransient);
            }
            AVAudioSessionInterruptionType::Ended => {
                let options = user_info_number(notification, unsafe {
                    AVAudioSessionInterruptionOptionKey
                        .expect("AVAudioSessionInterruptionOptionKey is present")
                })
                .map_or(
                    AVAudioSessionInterruptionOptions(0),
                    AVAudioSessionInterruptionOptions,
                );
                if options.contains(AVAudioSessionInterruptionOptions::ShouldResume) {
                    send_if_active(core, MediaCommand::AudioFocusGained);
                } else {
                    send_if_active(core, MediaCommand::AudioFocusLost);
                }
            }
            other => panic!("unsupported AVAudioSession interruption type {other:?}"),
        }
    }

    fn handle_route_change(core: &Arc<SessionCore>, notification: &NSNotification) {
        let reason = user_info_number(notification, unsafe {
            AVAudioSessionRouteChangeReasonKey
                .expect("AVAudioSessionRouteChangeReasonKey is present")
        })
        .map(AVAudioSessionRouteChangeReason);
        // newDeviceAvailable(1), categoryChange(3), override(4),
        // wakeFromSleep(5), noSuitableRouteForCategory(6),
        // routeConfigurationChange(7), unknown(0) — ignored as before.
        if reason == Some(AVAudioSessionRouteChangeReason::OldDeviceUnavailable) {
            send_if_active(core, MediaCommand::AudioBecomingNoisy);
        }
    }

    fn handle_silence_hint(core: &Arc<SessionCore>, notification: &NSNotification) {
        match user_info_number(notification, unsafe {
            AVAudioSessionSilenceSecondaryAudioHintTypeKey
                .expect("AVAudioSessionSilenceSecondaryAudioHintTypeKey is present")
        })
        .map(AVAudioSessionSilenceSecondaryAudioHintType)
        {
            Some(AVAudioSessionSilenceSecondaryAudioHintType::Begin) => {
                send_if_active(core, MediaCommand::AudioFocusLostDuck);
            }
            Some(AVAudioSessionSilenceSecondaryAudioHintType::End) => {
                send_if_active(core, MediaCommand::AudioFocusGained);
            }
            Some(other) => {
                panic!("unsupported AVAudioSession silence secondary audio hint type {other:?}")
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
    ) -> Retained<ProtocolObject<dyn objc2::runtime::NSObjectProtocol>> {
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

    /// Register the session's interruption/route-change/hint observers on
    /// the main queue. No main-thread answer is needed, so this hops with
    /// `exec_async` — `MediaSessionInner::new` stays synchronous.
    pub(super) fn register_system_event_observers(core: &Arc<SessionCore>) {
        let core = Arc::clone(core);
        dispatch2::DispatchQueue::main().exec_async(move || {
            let mtm = objc2::MainThreadMarker::new()
                .expect("exec_async closure must run on the main queue");
            // SAFETY: `sharedInstance`/`defaultCenter` return process
            // singletons; both are created here on the main queue and stay
            // local to this call.
            let session = unsafe { AVAudioSession::sharedInstance() };
            let center = NSNotificationCenter::defaultCenter();
            let tokens = dispatch2::MainThreadBound::new(
                std::sync::Mutex::new(vec![
                    observe(
                        &center,
                        &session,
                        unsafe {
                            AVAudioSessionInterruptionNotification
                                .expect("interruption notification is exported")
                        },
                        &core,
                        handle_interruption,
                    ),
                    observe(
                        &center,
                        &session,
                        unsafe {
                            AVAudioSessionRouteChangeNotification
                                .expect("route-change notification is exported")
                        },
                        &core,
                        handle_route_change,
                    ),
                    observe(
                        &center,
                        &session,
                        unsafe {
                            AVAudioSessionSilenceSecondaryAudioHintNotification
                                .expect("silence-hint notification is exported")
                        },
                        &core,
                        handle_silence_hint,
                    ),
                ]),
                mtm,
            );
            // Registered before `deactivate` can run `unregister`; a second
            // `set` only happens if the session was already closed.
            let _ = core.observer_tokens.set(tokens);
        });
    }

    pub(super) fn unregister_system_event_observers(core: &Arc<SessionCore>) {
        let Some(tokens) = core.observer_tokens.get() else {
            return;
        };
        tokens.get_on_main(|tokens| {
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
        let session = unsafe { AVAudioSession::sharedInstance() };
        let configured = unsafe {
            session.setCategory_mode_options_error(
                AVAudioSessionCategoryPlayback.expect("playback category is exported"),
                AVAudioSessionModeDefault.expect("default mode is exported"),
                AVAudioSessionCategoryOptions(0),
            )
        };
        configured
            .and_then(|()| unsafe { session.setActive_error(true) })
            .map_err(|_error| MediaError::AudioFocusDenied)
    }

    pub(super) fn abandon_audio_focus() -> Result<(), MediaError> {
        // SAFETY: `sharedInstance` returns the process-wide session.
        let session = unsafe { AVAudioSession::sharedInstance() };
        unsafe {
            session.setActive_withOptions_error(
                false,
                AVAudioSessionSetActiveOptions::NotifyOthersOnDeactivation,
            )
        }
        .map_err(|_error| MediaError::UpdateFailed("failed to deactivate AVAudioSession".into()))
    }
}

// ---------------------------------------------------------------------------
// macOS: AVAudioPlayer silent-activation pulse (absent from
// objc2-av-foundation 0.3.x — declared here)
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod macos {
    use objc2::{
        AnyThread, DeclaredClass, define_class, msg_send,
        rc::Retained,
        runtime::{NSObject, NSObjectProtocol, ProtocolObject},
    };
    use objc2_avf_audio::{AVAudioPlayer, AVAudioPlayerDelegate};
    use objc2_foundation::{NSData, NSError};
    use std::sync::Mutex;

    /// `AVAudioPlayer` is not `Send`/`Sync` in `objc2-avf-audio` 0.3.x, so
    /// the silent-pulse player is kept inside this wrapper.
    ///
    /// # Safety
    ///
    /// Apple documents `AVAudioPlayer` as thread-safe, and the delegate
    /// touches the player only through the ivar `Mutex`.
    struct SilentPlayer {
        _player: Retained<AVAudioPlayer>,
    }
    #[expect(
        clippy::non_send_fields_in_send_ty,
        reason = "AVAudioPlayer is thread-safe per Apple docs; accesses go through the ivar Mutex"
    )]
    unsafe impl Send for SilentPlayer {}
    /// # Safety
    ///
    /// See the `Send` impl.
    unsafe impl Sync for SilentPlayer {}

    pub struct SilentPlayerIvars {
        player: Mutex<Option<SilentPlayer>>,
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
            #[expect(
                non_snake_case,
                reason = "method name is fixed by the generated objc2 trait"
            )]
            #[unsafe(method(audioPlayerDidFinishPlaying:successfully:))]
            fn audioPlayerDidFinishPlaying_successfully(
                &self,
                _player: &AVAudioPlayer,
                _flag: bool,
            ) {
                self.ivars().player.lock().unwrap().take();
            }

            #[expect(
                non_snake_case,
                reason = "method name is fixed by the generated objc2 trait"
            )]
            #[unsafe(method(audioPlayerDecodeErrorDidOccur:error:))]
            fn audioPlayerDecodeErrorDidOccur_error(
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
        // SAFETY: `initWithData:error:` is a documented `AVAudioPlayer`
        // initializer; `data` is a complete WAV payload.
        let player =
            match unsafe { AVAudioPlayer::initWithData_error(AVAudioPlayer::alloc(), &data) } {
                Ok(player) => player,
                Err(error) => {
                    tracing::warn!(%error, "failed to create silent audio player");
                    return delegate;
                }
            };
        unsafe {
            player.setVolume(0.0);
            player.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            player.prepareToPlay();
            if !player.play() {
                tracing::warn!("silent audio activation pulse did not start playback");
                return delegate;
            }
        }
        *delegate.ivars().player.lock().unwrap() = Some(SilentPlayer { _player: player });
        delegate
    }
}

// ---------------------------------------------------------------------------
// iOS native audio player (AVPlayer) — compiled only with `playback`
// ---------------------------------------------------------------------------

#[cfg(all(target_os = "ios", feature = "playback"))]
mod player {
    use crate::{PlaybackStatus, PlayerError};
    use dispatch2::MainThreadBound;
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_av_foundation::{
        AVAudioTimePitchAlgorithmSpectral, AVAudioTimePitchAlgorithmVarispeed, AVPlayer,
        AVPlayerItem,
    };
    use objc2_avf_audio::{
        AVAudioSession, AVAudioSessionCategoryPlayback, AVAudioSessionModeDefault,
    };
    use objc2_core_media::CMTime;
    use objc2_foundation::{NSNumber, NSString, NSURL};
    use objc2_media_player::{
        MPMediaItemPropertyPlaybackDuration, MPNowPlayingInfoPropertyElapsedPlaybackTime,
        MPNowPlayingInfoPropertyPlaybackRate, MPNowPlayingPlaybackState,
    };
    use std::{
        sync::Mutex,
        sync::atomic::{AtomicBool, AtomicU32, Ordering},
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
        /// Construct the player shell; the main-thread `MainThreadBound` is
        /// created through `waterkit_core::apple::on_main`.
        pub fn new() -> Result<Self, PlayerError> {
            // SAFETY: `sharedInstance` returns the process-wide session.
            let session = unsafe { AVAudioSession::sharedInstance() };
            let configured = unsafe {
                session.setCategory_mode_options_error(
                    AVAudioSessionCategoryPlayback.expect("playback category is exported"),
                    AVAudioSessionModeDefault.expect("default mode is exported"),
                    objc2_avf_audio::AVAudioSessionCategoryOptions(0),
                )
            };
            configured
                .and_then(|()| unsafe { session.setActive_error(true) })
                .map_err(|error| {
                    tracing::error!(%error, "failed to activate AVAudioSession");
                    PlayerError::LoadFailed("Apple audio player failed to load media".into())
                })?;
            let player =
                dispatch2::run_on_main(|mtm| MainThreadBound::new(Mutex::new(None), mtm));
            Ok(Self {
                player,
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
            self.player.get_on_main(|slot| {
                let mtm = MainThreadMarker::new()
                    .expect("MainThreadBound::get_on_main runs on the main queue");
                let mut player_slot = slot.lock().unwrap();
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
                drop(player_slot);
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
