//! Windows media control implementation using `SystemMediaTransportControls`.

use crate::{MediaCommand, MediaError, MediaMetadata, PlaybackState, PlaybackStatus};
use windows::Foundation::TypedEventHandler;
use windows::Media::Playback::MediaPlayer;
use windows::Media::{
    MediaPlaybackStatus, MediaPlaybackType, SystemMediaTransportControls,
    SystemMediaTransportControlsButton, SystemMediaTransportControlsButtonPressedEventArgs,
};
use windows::Storage::Streams::{
    DataWriter, InMemoryRandomAccessStream, RandomAccessStreamReference,
};

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

    if let Some(artwork) = metadata.artwork() {
        let stream = InMemoryRandomAccessStream::new().map_err(win_err_update)?;
        let writer = DataWriter::CreateDataWriter(&stream).map_err(win_err_update)?;
        writer
            .WriteBytes(artwork.encoded())
            .map_err(win_err_update)?;
        let store = writer.StoreAsync().map_err(win_err_update)?;
        futures::executor::block_on(async move { store.await }).map_err(win_err_update)?;
        writer.DetachStream().map_err(win_err_update)?;
        stream.Seek(0).map_err(win_err_update)?;
        let stream_reference =
            RandomAccessStreamReference::CreateFromStream(&stream).map_err(win_err_update)?;
        updater
            .SetThumbnail(&stream_reference)
            .map_err(win_err_update)?;
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

fn setup_button_handler(
    controls: &SystemMediaTransportControls,
    command_sender: async_channel::Sender<MediaCommand>,
) -> Result<i64, MediaError> {
    let handler = TypedEventHandler::<
        SystemMediaTransportControls,
        SystemMediaTransportControlsButtonPressedEventArgs,
    >::new(move |_sender, args| {
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

            if let Some(command) = cmd
                && let Err(error) = command_sender.try_send(command)
            {
                tracing::warn!(%error, "failed to deliver Windows media command");
            }
        }
        Ok(())
    });

    controls
        .ButtonPressed(&handler)
        .map_err(|e| MediaError::Unknown(format!("{e}")))
}

#[derive(Debug)]
pub struct MediaSessionInner {
    _media_player: MediaPlayer,
    controls: SystemMediaTransportControls,
    button_handler: i64,
    command_receiver: async_channel::Receiver<MediaCommand>,
}

impl MediaSessionInner {
    pub fn new() -> Result<Self, MediaError> {
        let (media_player, controls) = create_controls()?;
        let (command_sender, command_receiver) = async_channel::unbounded();
        let button_handler = setup_button_handler(&controls, command_sender)?;
        Ok(Self {
            _media_player: media_player,
            controls,
            button_handler,
            command_receiver,
        })
    }

    pub fn set_metadata(&self, metadata: &MediaMetadata) -> Result<(), MediaError> {
        set_metadata_inner(&self.controls, metadata)
    }

    pub fn set_playback_state(&self, state: &PlaybackState) -> Result<(), MediaError> {
        set_playback_status_inner(&self.controls, state)
    }

    pub fn request_audio_focus(&self) -> Result<(), MediaError> {
        self.controls
            .IsEnabled()
            .map(|_| ())
            .map_err(win_err_update)
    }

    pub fn abandon_audio_focus(&self) -> Result<(), MediaError> {
        self.controls
            .IsEnabled()
            .map(|_| ())
            .map_err(win_err_update)
    }

    pub fn clear(&self) -> Result<(), MediaError> {
        let updater = self.controls.DisplayUpdater().map_err(win_err_update)?;
        updater.ClearAll().map_err(win_err_update)?;
        self.controls
            .SetPlaybackStatus(MediaPlaybackStatus::Closed)
            .map_err(win_err_update)?;
        Ok(())
    }

    pub fn command_receiver(&self) -> async_channel::Receiver<MediaCommand> {
        self.command_receiver.clone()
    }
}

impl Drop for MediaSessionInner {
    fn drop(&mut self) {
        if let Err(error) = self.controls.RemoveButtonPressed(self.button_handler) {
            tracing::error!(%error, "failed to remove Windows media command handler");
        }
        if let Err(error) = self.clear() {
            tracing::error!(%error, "failed to clear Windows media session during shutdown");
        }
    }
}
