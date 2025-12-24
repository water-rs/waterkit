//! Windows media control implementation using SystemMediaTransportControls.

use crate::{
    MediaCommand, MediaCommandHandler, MediaError, MediaMetadata, PlaybackState, PlaybackStatus,
};
use std::sync::RwLock;
use windows::Foundation::TypedEventHandler;
use windows::Media::Playback::{MediaPlaybackType, MediaPlayer};
use windows::Media::{
    MediaPlaybackAutoRepeatMode, MediaPlaybackStatus, MediaPlaybackType as MPType,
    SystemMediaTransportControls, SystemMediaTransportControlsButton,
    SystemMediaTransportControlsButtonPressedEventArgs,
};

/// Global command handler
static COMMAND_HANDLER: RwLock<Option<Box<dyn MediaCommandHandler>>> = RwLock::new(None);

#[derive(Debug)]
pub struct MediaSessionInner {
    media_player: MediaPlayer,
    controls: SystemMediaTransportControls,
}

impl MediaSessionInner {
    pub fn new() -> Result<Self, MediaError> {
        let media_player = MediaPlayer::new()
            .map_err(|e| MediaError::InitializationFailed(e.message().to_string()))?;

        let controls = media_player
            .SystemMediaTransportControls()
            .map_err(|e| MediaError::InitializationFailed(e.message().to_string()))?;

        // Enable controls
        controls
            .SetIsEnabled(true)
            .map_err(|e| MediaError::InitializationFailed(e.message().to_string()))?;
        controls
            .SetIsPlayEnabled(true)
            .map_err(|e| MediaError::InitializationFailed(e.message().to_string()))?;
        controls
            .SetIsPauseEnabled(true)
            .map_err(|e| MediaError::InitializationFailed(e.message().to_string()))?;
        controls
            .SetIsStopEnabled(true)
            .map_err(|e| MediaError::InitializationFailed(e.message().to_string()))?;
        controls
            .SetIsNextEnabled(true)
            .map_err(|e| MediaError::InitializationFailed(e.message().to_string()))?;
        controls
            .SetIsPreviousEnabled(true)
            .map_err(|e| MediaError::InitializationFailed(e.message().to_string()))?;

        Ok(Self {
            media_player,
            controls,
        })
    }

    pub fn set_metadata(&self, metadata: &MediaMetadata) -> Result<(), MediaError> {
        let updater = self
            .controls
            .DisplayUpdater()
            .map_err(|e| MediaError::UpdateFailed(e.message().to_string()))?;

        updater
            .SetType(MPType::Music)
            .map_err(|e| MediaError::UpdateFailed(e.message().to_string()))?;

        let music_props = updater
            .MusicProperties()
            .map_err(|e| MediaError::UpdateFailed(e.message().to_string()))?;

        if let Some(ref title) = metadata.title {
            music_props
                .SetTitle(&windows::core::HSTRING::from(title.as_str()))
                .map_err(|e| MediaError::UpdateFailed(e.message().to_string()))?;
        }

        if let Some(ref artist) = metadata.artist {
            music_props
                .SetArtist(&windows::core::HSTRING::from(artist.as_str()))
                .map_err(|e| MediaError::UpdateFailed(e.message().to_string()))?;
        }

        if let Some(ref album) = metadata.album {
            music_props
                .SetAlbumTitle(&windows::core::HSTRING::from(album.as_str()))
                .map_err(|e| MediaError::UpdateFailed(e.message().to_string()))?;
        }

        // Artwork from URL
        if let Some(ref url) = metadata.artwork_url {
            if let Ok(uri) =
                windows::Foundation::Uri::CreateUri(&windows::core::HSTRING::from(url.as_str()))
            {
                if let Ok(stream) =
                    windows::Storage::Streams::RandomAccessStreamReference::CreateFromUri(&uri)
                {
                    let _ = updater.SetThumbnail(&stream);
                }
            }
        }

        updater
            .Update()
            .map_err(|e| MediaError::UpdateFailed(e.message().to_string()))?;

        Ok(())
    }

    pub fn set_playback_state(&self, state: &PlaybackState) -> Result<(), MediaError> {
        let status = match state.status {
            PlaybackStatus::Playing => MediaPlaybackStatus::Playing,
            PlaybackStatus::Paused => MediaPlaybackStatus::Paused,
            PlaybackStatus::Stopped => MediaPlaybackStatus::Stopped,
        };

        self.controls
            .SetPlaybackStatus(status)
            .map_err(|e| MediaError::UpdateFailed(e.message().to_string()))?;

        Ok(())
    }

    pub fn set_command_handler(
        &self,
        handler: Box<dyn MediaCommandHandler>,
    ) -> Result<(), MediaError> {
        {
            let mut guard = COMMAND_HANDLER
                .write()
                .map_err(|e| MediaError::Unknown(format!("Lock poisoned: {e}")))?;
            *guard = Some(handler);
        }

        let handler = TypedEventHandler::new(
            |_sender: &Option<SystemMediaTransportControls>,
             args: &Option<SystemMediaTransportControlsButtonPressedEventArgs>| {
                if let Some(args) = args {
                    if let Ok(button) = args.Button() {
                        let cmd = match button {
                            SystemMediaTransportControlsButton::Play => Some(MediaCommand::Play),
                            SystemMediaTransportControlsButton::Pause => Some(MediaCommand::Pause),
                            SystemMediaTransportControlsButton::Stop => Some(MediaCommand::Stop),
                            SystemMediaTransportControlsButton::Next => Some(MediaCommand::Next),
                            SystemMediaTransportControlsButton::Previous => {
                                Some(MediaCommand::Previous)
                            }
                            _ => None,
                        };

                        if let Some(cmd) = cmd {
                            if let Ok(guard) = COMMAND_HANDLER.read() {
                                if let Some(handler) = guard.as_ref() {
                                    handler.on_command(cmd);
                                }
                            }
                        }
                    }
                }
                Ok(())
            },
        );

        self.controls
            .ButtonPressed(&handler)
            .map_err(|e| MediaError::Unknown(e.message().to_string()))?;

        Ok(())
    }

    pub fn request_audio_focus(&self) -> Result<(), MediaError> {
        // Windows doesn't have an explicit audio focus API like Android
        // The SMTC handles this automatically
        Ok(())
    }

    pub fn abandon_audio_focus(&self) -> Result<(), MediaError> {
        // Windows doesn't have an explicit audio focus API
        Ok(())
    }

    pub fn clear(&self) -> Result<(), MediaError> {
        let updater = self
            .controls
            .DisplayUpdater()
            .map_err(|e| MediaError::UpdateFailed(e.message().to_string()))?;

        updater
            .ClearAll()
            .map_err(|e| MediaError::UpdateFailed(e.message().to_string()))?;

        self.controls
            .SetPlaybackStatus(MediaPlaybackStatus::Closed)
            .map_err(|e| MediaError::UpdateFailed(e.message().to_string()))?;

        Ok(())
    }
}
