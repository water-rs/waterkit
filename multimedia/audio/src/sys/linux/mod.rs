//! Linux media control implementation using an instance-owned MPRIS D-Bus service.

use crate::{
    MediaCommand, MediaError, MediaMetadata, PlaybackState, PlaybackStatus, QueueNavigationControls,
};
use std::{
    collections::HashMap,
    fmt::Display,
    io::Write,
    sync::{Arc, RwLock},
    thread::JoinHandle,
    time::Duration,
};
use zbus::{
    Connection,
    connection::Builder as ConnectionBuilder,
    interface,
    zvariant::{ObjectPath, Value},
};

#[derive(Debug)]
struct MprisState {
    metadata: RwLock<HashMap<String, Value<'static>>>,
    artwork_file: RwLock<Option<tempfile::NamedTempFile>>,
    status: RwLock<PlaybackStatus>,
    position_micros: RwLock<i64>,
    queue_navigation: RwLock<QueueNavigationControls>,
    command_sender: async_channel::Sender<MediaCommand>,
}

impl MprisState {
    fn new(command_sender: async_channel::Sender<MediaCommand>) -> Self {
        Self {
            metadata: RwLock::new(HashMap::new()),
            artwork_file: RwLock::new(None),
            status: RwLock::new(PlaybackStatus::Stopped),
            position_micros: RwLock::new(0),
            queue_navigation: RwLock::new(QueueNavigationControls::default()),
            command_sender,
        }
    }

    fn send(&self, command: MediaCommand) {
        if let Err(error) = self.command_sender.try_send(command) {
            tracing::warn!(%error, "failed to deliver MPRIS media command");
        }
    }

    fn set_metadata(&self, metadata: &MediaMetadata) -> Result<(), MediaError> {
        let mut values = HashMap::new();
        values.insert(
            String::from("mpris:trackid"),
            Value::new(
                ObjectPath::try_from("/org/waterkit/media/track")
                    .expect("the WaterKit MPRIS track path must be valid"),
            ),
        );
        if let Some(title) = metadata.title() {
            values.insert(String::from("xesam:title"), Value::new(title.to_owned()));
        }
        if let Some(artist) = metadata.artist() {
            values.insert(
                String::from("xesam:artist"),
                Value::new(vec![artist.to_owned()]),
            );
        }
        if let Some(album) = metadata.album() {
            values.insert(String::from("xesam:album"), Value::new(album.to_owned()));
        }
        let artwork_file = if let Some(artwork) = metadata.artwork() {
            let mut file = tempfile::NamedTempFile::new().map_err(|error| {
                MediaError::UpdateFailed(format!("failed to create MPRIS artwork file: {error}"))
            })?;
            file.write_all(artwork.encoded()).map_err(|error| {
                MediaError::UpdateFailed(format!("failed to write MPRIS artwork: {error}"))
            })?;
            file.flush().map_err(|error| {
                MediaError::UpdateFailed(format!("failed to flush MPRIS artwork: {error}"))
            })?;
            let url = url::Url::from_file_path(file.path()).map_err(|()| {
                MediaError::UpdateFailed(format!(
                    "failed to convert MPRIS artwork path to URL: {}",
                    file.path().display()
                ))
            })?;
            values.insert(String::from("mpris:artUrl"), Value::new(String::from(url)));
            Some(file)
        } else {
            None
        };
        if let Some(duration) = metadata.duration() {
            values.insert(
                String::from("mpris:length"),
                Value::new(duration_micros_i64(duration)?),
            );
        }
        *self
            .metadata
            .write()
            .map_err(|error| poisoned_lock("metadata", error))? = values;
        *self
            .artwork_file
            .write()
            .map_err(|error| poisoned_lock("artwork file", error))? = artwork_file;
        Ok(())
    }

    fn set_playback_state(&self, state: &PlaybackState) -> Result<(), MediaError> {
        *self
            .status
            .write()
            .map_err(|error| poisoned_lock("playback status", error))? = state.status();
        if let Some(position) = state.position() {
            *self
                .position_micros
                .write()
                .map_err(|error| poisoned_lock("position", error))? =
                duration_micros_i64(position)?;
        }
        *self
            .queue_navigation
            .write()
            .map_err(|error| poisoned_lock("queue navigation", error))? =
            state.queue_navigation_controls();
        Ok(())
    }

    fn clear(&self) -> Result<(), MediaError> {
        self.metadata
            .write()
            .map_err(|error| poisoned_lock("metadata", error))?
            .clear();
        self.artwork_file
            .write()
            .map_err(|error| poisoned_lock("artwork file", error))?
            .take();
        *self
            .status
            .write()
            .map_err(|error| poisoned_lock("playback status", error))? = PlaybackStatus::Stopped;
        *self
            .queue_navigation
            .write()
            .map_err(|error| poisoned_lock("queue navigation", error))? =
            QueueNavigationControls::default();
        Ok(())
    }
}

struct MediaPlayer2;

#[allow(
    clippy::missing_const_for_fn,
    clippy::unused_self,
    reason = "zbus interface methods must be ordinary instance methods generated through #[interface]."
)]
#[interface(name = "org.mpris.MediaPlayer2")]
impl MediaPlayer2 {
    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn identity(&self) -> String {
        String::from("WaterKit Media")
    }

    #[zbus(property)]
    fn desktop_entry(&self) -> String {
        String::from("waterkit")
    }

    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }

    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }

    fn raise(&self) -> zbus::fdo::Result<()> {
        Err(zbus::fdo::Error::NotSupported(String::from(
            "WaterKit does not own an application window to raise",
        )))
    }

    fn quit(&self) -> zbus::fdo::Result<()> {
        Err(zbus::fdo::Error::NotSupported(String::from(
            "WaterKit does not own the host application lifecycle",
        )))
    }
}

struct MprisPlayer {
    state: Arc<MprisState>,
}

#[allow(
    clippy::missing_const_for_fn,
    clippy::unused_self,
    reason = "zbus interface methods must be ordinary instance methods generated through #[interface]."
)]
#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl MprisPlayer {
    #[zbus(property)]
    fn playback_status(&self) -> String {
        match *self
            .state
            .status
            .read()
            .expect("MPRIS playback status lock must not be poisoned")
        {
            PlaybackStatus::Playing => String::from("Playing"),
            PlaybackStatus::Paused => String::from("Paused"),
            PlaybackStatus::Stopped => String::from("Stopped"),
        }
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, Value<'static>> {
        self.state
            .metadata
            .read()
            .expect("MPRIS metadata lock must not be poisoned")
            .clone()
    }

    #[zbus(property)]
    fn position(&self) -> i64 {
        self.state
            .position_micros
            .read()
            .map(|position| *position)
            .expect("MPRIS position lock must not be poisoned")
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        self.state
            .queue_navigation
            .read()
            .expect("MPRIS queue-navigation lock must not be poisoned")
            .next_enabled()
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        self.state
            .queue_navigation
            .read()
            .expect("MPRIS queue-navigation lock must not be poisoned")
            .previous_enabled()
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }

    fn next(&self) {
        self.state.send(MediaCommand::Next);
    }

    fn previous(&self) {
        self.state.send(MediaCommand::Previous);
    }

    fn pause(&self) {
        self.state.send(MediaCommand::Pause);
    }

    fn play_pause(&self) {
        self.state.send(MediaCommand::PlayPause);
    }

    fn stop(&self) {
        self.state.send(MediaCommand::Stop);
    }

    fn play(&self) {
        self.state.send(MediaCommand::Play);
    }

    fn seek(&self, offset: i64) {
        let duration = Duration::from_micros(offset.unsigned_abs());
        self.state.send(if offset >= 0 {
            MediaCommand::SeekForward(duration)
        } else {
            MediaCommand::SeekBackward(duration)
        });
    }

    #[allow(
        clippy::needless_pass_by_value,
        reason = "zbus owns D-Bus method arguments when generating #[interface] dispatch glue."
    )]
    fn set_position(&self, track_id: ObjectPath<'_>, position: i64) -> zbus::fdo::Result<()> {
        let duration = Duration::from_micros(u64::try_from(position).map_err(|_| {
            zbus::fdo::Error::InvalidArgs(format!(
                "MPRIS SetPosition for {track_id} received a negative position: {position}"
            ))
        })?);
        self.state.send(MediaCommand::Seek(duration));
        Ok(())
    }

    #[allow(
        clippy::needless_pass_by_value,
        reason = "zbus owns D-Bus method arguments when generating #[interface] dispatch glue"
    )]
    fn open_uri(&self, uri: String) -> zbus::fdo::Result<()> {
        Err(zbus::fdo::Error::NotSupported(format!(
            "WaterKit's player instance does not accept MPRIS OpenUri requests: {uri}"
        )))
    }
}

#[derive(Debug)]
pub struct MediaSessionInner {
    state: Arc<MprisState>,
    command_receiver: async_channel::Receiver<MediaCommand>,
    stop: async_channel::Sender<()>,
    service_thread: Option<JoinHandle<()>>,
}

impl MediaSessionInner {
    pub fn new() -> Result<Self, MediaError> {
        let (command_sender, command_receiver) = async_channel::unbounded();
        let state = Arc::new(MprisState::new(command_sender));
        let (stop, stop_receiver) = async_channel::bounded(1);
        let (started_sender, started_receiver) = std::sync::mpsc::sync_channel(1);
        let service_state = Arc::clone(&state);
        let service_thread = std::thread::spawn(move || {
            smol::block_on(async move {
                match start_dbus_service(service_state).await {
                    Ok(connection) => {
                        if started_sender.send(Ok(())).is_ok() {
                            let _ = stop_receiver.recv().await;
                        }
                        drop(connection);
                    }
                    Err(error) => {
                        let _ = started_sender.send(Err(error.to_string()));
                    }
                }
            });
        });
        started_receiver
            .recv()
            .map_err(|_| {
                MediaError::InitializationFailed(String::from(
                    "MPRIS service thread ended before reporting readiness",
                ))
            })?
            .map_err(MediaError::InitializationFailed)?;
        Ok(Self {
            state,
            command_receiver,
            stop,
            service_thread: Some(service_thread),
        })
    }

    pub fn set_metadata(&self, metadata: &MediaMetadata) -> Result<(), MediaError> {
        self.state.set_metadata(metadata)
    }

    pub fn set_playback_state(&self, state: &PlaybackState) -> Result<(), MediaError> {
        self.state.set_playback_state(state)
    }

    #[allow(
        clippy::missing_const_for_fn,
        clippy::unnecessary_wraps,
        clippy::unused_self,
        reason = "Linux has no centralized audio-focus service."
    )]
    pub fn request_audio_focus(&self) -> Result<(), MediaError> {
        Ok(())
    }

    #[allow(
        clippy::missing_const_for_fn,
        clippy::unnecessary_wraps,
        clippy::unused_self,
        reason = "Linux has no centralized audio-focus service."
    )]
    pub fn abandon_audio_focus(&self) -> Result<(), MediaError> {
        Ok(())
    }

    pub fn clear(&self) -> Result<(), MediaError> {
        self.state.clear()
    }

    pub fn command_receiver(&self) -> async_channel::Receiver<MediaCommand> {
        self.command_receiver.clone()
    }
}

impl Drop for MediaSessionInner {
    fn drop(&mut self) {
        if let Err(error) = self.clear() {
            tracing::error!(%error, "failed to clear MPRIS media session during shutdown");
        }
        self.stop.close();
        if let Some(service_thread) = self.service_thread.take() {
            service_thread
                .join()
                .expect("MPRIS service thread must not panic during shutdown");
        }
    }
}

fn duration_micros_i64(duration: Duration) -> Result<i64, MediaError> {
    i64::try_from(duration.as_micros()).map_err(|_| {
        MediaError::UpdateFailed(format!(
            "duration {duration:?} exceeds the MPRIS i64 microsecond range"
        ))
    })
}

fn poisoned_lock(context: &str, error: impl Display) -> MediaError {
    MediaError::Unknown(format!("{context} lock poisoned: {error}"))
}

async fn start_dbus_service(state: Arc<MprisState>) -> Result<Connection, zbus::Error> {
    ConnectionBuilder::session()?
        .name("org.mpris.MediaPlayer2.waterkit")?
        .serve_at("/org/mpris/MediaPlayer2", MediaPlayer2)?
        .serve_at("/org/mpris/MediaPlayer2", MprisPlayer { state })?
        .build()
        .await
}
