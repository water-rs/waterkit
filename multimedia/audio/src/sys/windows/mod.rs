//! Windows media control implementation using `SystemMediaTransportControls`.

use crate::{MediaCommand, MediaError, MediaMetadata, PlaybackState, PlaybackStatus};
use std::sync::RwLock;
use windows::Foundation::TypedEventHandler;
use windows::Media::Playback::MediaPlayer;
use windows::Media::{
    MediaPlaybackStatus, MediaPlaybackType, SystemMediaTransportControls,
    SystemMediaTransportControlsButton, SystemMediaTransportControlsButtonPressedEventArgs,
};

/// Pending commands queue
static PENDING_COMMANDS: RwLock<Vec<MediaCommand>> = RwLock::new(Vec::new());

#[allow(clippy::needless_pass_by_value)]
fn win_err_update(e: windows::core::Error) -> MediaError {
    MediaError::UpdateFailed(e.to_string())
}

#[allow(clippy::needless_pass_by_value)]
fn win_err_init(e: windows::core::Error) -> MediaError {
    MediaError::InitializationFailed(e.to_string())
}

fn set_metadata_inner(
    controls: &SystemMediaTransportControls,
    metadata: &MediaMetadata,
) -> Result<(), MediaError> {
    let updater = controls.DisplayUpdater().map_err(win_err_update)?;

    updater
        .SetType(MediaPlaybackType::Music)
        .map_err(win_err_update)?;

    let music_props = updater.MusicProperties().map_err(win_err_update)?;

    if let Some(title) = metadata.title() {
        music_props
            .SetTitle(&windows::core::HSTRING::from(title))
            .map_err(win_err_update)?;
    }

    if let Some(artist) = metadata.artist() {
        music_props
            .SetArtist(&windows::core::HSTRING::from(artist))
            .map_err(win_err_update)?;
    }

    if let Some(album) = metadata.album() {
        music_props
            .SetAlbumTitle(&windows::core::HSTRING::from(album))
            .map_err(win_err_update)?;
    }

    if let Some(url) = metadata.artwork_url()
        && let Ok(uri) = windows::Foundation::Uri::CreateUri(&windows::core::HSTRING::from(url))
        && let Ok(stream) =
            windows::Storage::Streams::RandomAccessStreamReference::CreateFromUri(&uri)
    {
        let _ = updater.SetThumbnail(&stream);
    }

    updater.Update().map_err(win_err_update)?;

    Ok(())
}

fn set_playback_status_inner(
    controls: &SystemMediaTransportControls,
    state: &PlaybackState,
) -> Result<(), MediaError> {
    let status = match state.status() {
        PlaybackStatus::Playing => MediaPlaybackStatus::Playing,
        PlaybackStatus::Paused => MediaPlaybackStatus::Paused,
        PlaybackStatus::Stopped => MediaPlaybackStatus::Stopped,
    };

    controls.SetPlaybackStatus(status).map_err(win_err_update)?;
    controls
        .SetIsNextEnabled(state.queue_navigation_controls().next_enabled())
        .map_err(win_err_update)?;
    controls
        .SetIsPreviousEnabled(state.queue_navigation_controls().previous_enabled())
        .map_err(win_err_update)?;

    Ok(())
}

fn create_controls() -> Result<(MediaPlayer, SystemMediaTransportControls), MediaError> {
    let media_player = MediaPlayer::new().map_err(win_err_init)?;

    let controls = media_player
        .SystemMediaTransportControls()
        .map_err(win_err_init)?;

    controls.SetIsEnabled(true).map_err(win_err_init)?;
    controls.SetIsPlayEnabled(true).map_err(win_err_init)?;
    controls.SetIsPauseEnabled(true).map_err(win_err_init)?;
    controls.SetIsStopEnabled(true).map_err(win_err_init)?;
    controls.SetIsNextEnabled(false).map_err(win_err_init)?;
    controls.SetIsPreviousEnabled(false).map_err(win_err_init)?;

    Ok((media_player, controls))
}

fn setup_button_handler(controls: &SystemMediaTransportControls) -> Result<(), MediaError> {
    let handler = TypedEventHandler::<
        SystemMediaTransportControls,
        SystemMediaTransportControlsButtonPressedEventArgs,
    >::new(|_sender, args| {
        if let Some(args) = args.as_ref()
            && let Ok(button) = args.Button()
        {
            let cmd = match button {
                SystemMediaTransportControlsButton::Play => Some(MediaCommand::Play),
                SystemMediaTransportControlsButton::Pause => Some(MediaCommand::Pause),
                SystemMediaTransportControlsButton::Stop => Some(MediaCommand::Stop),
                SystemMediaTransportControlsButton::Next => Some(MediaCommand::Next),
                SystemMediaTransportControlsButton::Previous => Some(MediaCommand::Previous),
                _ => None,
            };

            if let Some(cmd) = cmd {
                if let Ok(mut guard) = PENDING_COMMANDS.write() {
                    guard.push(cmd);
                }
            }
        }
        Ok(())
    });

    controls
        .ButtonPressed(&handler)
        .map_err(|e| MediaError::Unknown(format!("{e}")))?;

    Ok(())
}

// -- MediaSessionInner: legacy API used by MediaSession --

#[derive(Debug)]
pub struct MediaSessionInner {
    #[allow(dead_code)]
    media_player: MediaPlayer,
    controls: SystemMediaTransportControls,
}

#[allow(
    dead_code,
    clippy::needless_pass_by_value,
    clippy::unused_self,
    clippy::missing_const_for_fn,
    clippy::unnecessary_wraps
)]
impl MediaSessionInner {
    pub fn new() -> Result<Self, MediaError> {
        let (media_player, controls) = create_controls()?;
        setup_button_handler(&controls)?;
        Ok(Self {
            media_player,
            controls,
        })
    }

    pub fn set_metadata(&self, metadata: &MediaMetadata) -> Result<(), MediaError> {
        set_metadata_inner(&self.controls, metadata)
    }

    pub fn set_playback_state(&self, state: &PlaybackState) -> Result<(), MediaError> {
        set_playback_status_inner(&self.controls, state)
    }

    pub fn request_audio_focus(&self) -> Result<(), MediaError> {
        Ok(())
    }

    pub fn abandon_audio_focus(&self) -> Result<(), MediaError> {
        Ok(())
    }

    pub fn clear(&self) -> Result<(), MediaError> {
        let updater = self.controls.DisplayUpdater().map_err(win_err_update)?;
        updater.ClearAll().map_err(win_err_update)?;
        self.controls
            .SetPlaybackStatus(MediaPlaybackStatus::Closed)
            .map_err(win_err_update)?;
        Ok(())
    }

    #[allow(clippy::unused_self)]
    pub fn poll_command(&self) -> Option<MediaCommand> {
        PENDING_COMMANDS.write().ok()?.pop()
    }
}

// -- MediaCenterInner: simplified API used by MediaCenterIntegration --

#[derive(Debug)]
pub struct MediaCenterInner {
    #[allow(dead_code)]
    media_player: MediaPlayer,
    controls: SystemMediaTransportControls,
}

impl MediaCenterInner {
    pub fn new() -> Result<Self, MediaError> {
        let (media_player, controls) = create_controls()?;
        setup_button_handler(&controls)?;
        Ok(Self {
            media_player,
            controls,
        })
    }

    pub fn update(&self, metadata: &MediaMetadata, state: &PlaybackState) {
        let _ = set_metadata_inner(&self.controls, metadata);
        let _ = set_playback_status_inner(&self.controls, state);
    }

    pub fn clear(&self) {
        if let Ok(updater) = self.controls.DisplayUpdater() {
            let _ = updater.ClearAll();
        }
        let _ = self.controls.SetPlaybackStatus(MediaPlaybackStatus::Closed);
    }

    #[allow(clippy::unused_self)]
    pub fn run_loop(&self, duration: std::time::Duration) {
        std::thread::sleep(duration);
    }

    #[allow(clippy::unused_self)]
    pub fn poll_command(&self) -> Option<MediaCommand> {
        PENDING_COMMANDS.write().ok()?.pop()
    }
}
