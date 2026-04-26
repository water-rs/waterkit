//! Linux media control implementation using MPRIS D-Bus.

use crate::{
    MediaCommand, MediaError, MediaMetadata, PlaybackState, PlaybackStatus, QueueNavigationControls,
};
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;
use zbus::zvariant::{ObjectPath, Value};
use zbus::{Connection, connection::Builder as ConnectionBuilder, interface};

/// Pending commands for polling-based media session integration.
static PENDING_COMMANDS: LazyLock<RwLock<Vec<MediaCommand>>> =
    LazyLock::new(|| RwLock::new(Vec::new()));

/// Current metadata for MPRIS properties
static CURRENT_METADATA: LazyLock<RwLock<HashMap<String, Value<'static>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Current playback status
static CURRENT_STATUS: LazyLock<RwLock<PlaybackStatus>> =
    LazyLock::new(|| RwLock::new(PlaybackStatus::Stopped));

/// Current position in microseconds
static CURRENT_POSITION: LazyLock<RwLock<i64>> = LazyLock::new(|| RwLock::new(0));

/// Current queue navigation capabilities.
static CURRENT_QUEUE_NAVIGATION: LazyLock<RwLock<QueueNavigationControls>> =
    LazyLock::new(|| RwLock::new(QueueNavigationControls::default()));

/// MPRIS `MediaPlayer2` interface implementation
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
        "WaterKit Media".to_string()
    }

    #[zbus(property)]
    fn desktop_entry(&self) -> String {
        "waterkit".to_string()
    }

    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        vec![]
    }

    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        vec![]
    }

    fn raise(&self) {}
    fn quit(&self) {}
}

/// MPRIS Player interface implementation
struct MprisPlayer;

#[allow(
    clippy::missing_const_for_fn,
    clippy::unused_self,
    reason = "zbus interface methods must be ordinary instance methods generated through #[interface]."
)]
#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl MprisPlayer {
    #[zbus(property)]
    fn playback_status(&self) -> String {
        let status = CURRENT_STATUS
            .read()
            .map_or(PlaybackStatus::Stopped, |s| *s);
        match status {
            PlaybackStatus::Playing => "Playing".to_string(),
            PlaybackStatus::Paused => "Paused".to_string(),
            PlaybackStatus::Stopped => "Stopped".to_string(),
        }
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, Value<'static>> {
        CURRENT_METADATA
            .read()
            .map_or_else(|_| HashMap::new(), |m| m.clone())
    }

    #[zbus(property)]
    fn position(&self) -> i64 {
        CURRENT_POSITION.read().map_or(0, |p| *p)
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
        CURRENT_QUEUE_NAVIGATION
            .read()
            .is_ok_and(|controls| controls.next_enabled())
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        CURRENT_QUEUE_NAVIGATION
            .read()
            .is_ok_and(|controls| controls.previous_enabled())
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
        dispatch_command(MediaCommand::Next);
    }

    fn previous(&self) {
        dispatch_command(MediaCommand::Previous);
    }

    fn pause(&self) {
        dispatch_command(MediaCommand::Pause);
    }

    fn play_pause(&self) {
        dispatch_command(MediaCommand::PlayPause);
    }

    fn stop(&self) {
        dispatch_command(MediaCommand::Stop);
    }

    fn play(&self) {
        dispatch_command(MediaCommand::Play);
    }

    fn seek(&self, offset: i64) {
        let duration = Duration::from_micros(offset.unsigned_abs());
        if offset >= 0 {
            dispatch_command(MediaCommand::SeekForward(duration));
        } else {
            dispatch_command(MediaCommand::SeekBackward(duration));
        }
    }

    fn set_position(&self, track_id: ObjectPath<'_>, position: i64) -> zbus::fdo::Result<()> {
        let duration = Duration::from_micros(u64::try_from(position).map_err(|_| {
            zbus::fdo::Error::InvalidArgs(format!(
                "MPRIS SetPosition for {track_id} received a negative position: {position}"
            ))
        })?);
        dispatch_command(MediaCommand::Seek(duration));
        Ok(())
    }

    fn open_uri(&self, uri: String) {
        drop(uri);
        // Not implemented
    }
}

fn dispatch_command(cmd: MediaCommand) {
    if let Ok(mut queue) = PENDING_COMMANDS.write() {
        queue.push(cmd.clone());
    }
}

#[derive(Debug)]
pub struct MediaSessionInner;

impl MediaSessionInner {
    #[allow(
        clippy::unnecessary_wraps,
        reason = "MediaSessionInner::new has a cross-platform fallible constructor shape; Linux reports D-Bus startup failures asynchronously."
    )]
    pub fn new() -> Result<Self, MediaError> {
        // Start the D-Bus service in a background thread
        std::thread::spawn(move || {
            smol::block_on(async {
                match start_dbus_service().await {
                    Ok(_connection) => {
                        // Keep the connection alive
                        std::future::pending::<()>().await;
                    }
                    Err(e) => {
                        tracing::error!(%e, "failed to start MPRIS service");
                    }
                }
            });
        });

        Ok(Self)
    }

    #[allow(
        clippy::unused_self,
        reason = "Linux MPRIS state is process-global because D-Bus exposes a single well-known media-player name."
    )]
    pub fn set_metadata(&self, metadata: &MediaMetadata) -> Result<(), MediaError> {
        let mut mpris_metadata: HashMap<String, Value<'static>> = HashMap::new();

        // Track ID is required
        mpris_metadata.insert(
            "mpris:trackid".to_string(),
            Value::new(ObjectPath::try_from("/org/waterkit/media/track").unwrap()),
        );

        if let Some(title) = metadata.title() {
            mpris_metadata.insert("xesam:title".to_string(), Value::new(title.to_owned()));
        }

        if let Some(artist) = metadata.artist() {
            mpris_metadata.insert(
                "xesam:artist".to_string(),
                Value::new(vec![artist.to_owned()]),
            );
        }

        if let Some(album) = metadata.album() {
            mpris_metadata.insert("xesam:album".to_string(), Value::new(album.to_owned()));
        }

        if let Some(url) = metadata.artwork_url() {
            mpris_metadata.insert("mpris:artUrl".to_string(), Value::new(url.to_owned()));
        }

        if let Some(duration) = metadata.duration() {
            mpris_metadata.insert(
                "mpris:length".to_string(),
                Value::new(duration_micros_i64(duration)?),
            );
        }

        let mut guard = CURRENT_METADATA
            .write()
            .map_err(|error| poisoned_lock("metadata", error))?;
        *guard = mpris_metadata;

        Ok(())
    }

    #[allow(
        clippy::unused_self,
        reason = "Linux MPRIS state is process-global because D-Bus exposes a single well-known media-player name."
    )]
    pub fn set_playback_state(&self, state: &PlaybackState) -> Result<(), MediaError> {
        let mut status = CURRENT_STATUS
            .write()
            .map_err(|error| poisoned_lock("playback status", error))?;
        *status = state.status();
        drop(status);

        if let Some(position) = state.position() {
            let mut guard = CURRENT_POSITION
                .write()
                .map_err(|error| poisoned_lock("position", error))?;
            *guard = duration_micros_i64(position)?;
        }

        let mut queue_navigation = CURRENT_QUEUE_NAVIGATION
            .write()
            .map_err(|error| poisoned_lock("queue navigation", error))?;
        *queue_navigation = state.queue_navigation_controls();

        Ok(())
    }

    #[allow(
        clippy::missing_const_for_fn,
        clippy::unnecessary_wraps,
        clippy::unused_self,
        reason = "Linux has no centralized audio-focus API, but MediaSession keeps the cross-platform fallible API shape."
    )]
    pub fn request_audio_focus(&self) -> Result<(), MediaError> {
        // Linux doesn't have a centralized audio focus system
        Ok(())
    }

    #[allow(
        clippy::missing_const_for_fn,
        clippy::unnecessary_wraps,
        clippy::unused_self,
        reason = "Linux has no centralized audio-focus API, but MediaSession keeps the cross-platform fallible API shape."
    )]
    pub fn abandon_audio_focus(&self) -> Result<(), MediaError> {
        Ok(())
    }

    #[allow(
        clippy::unused_self,
        reason = "Linux MPRIS state is process-global because D-Bus exposes a single well-known media-player name."
    )]
    pub fn clear(&self) -> Result<(), MediaError> {
        CURRENT_METADATA
            .write()
            .map_err(|error| poisoned_lock("metadata", error))?
            .clear();
        *CURRENT_STATUS
            .write()
            .map_err(|error| poisoned_lock("playback status", error))? = PlaybackStatus::Stopped;
        *CURRENT_QUEUE_NAVIGATION
            .write()
            .map_err(|error| poisoned_lock("queue navigation", error))? =
            QueueNavigationControls::default();
        Ok(())
    }

    #[allow(
        clippy::unused_self,
        reason = "Linux MPRIS commands are queued from the process-global D-Bus interface."
    )]
    pub fn poll_command(&self) -> Option<crate::MediaCommand> {
        PENDING_COMMANDS.write().ok()?.pop()
    }
}

/// Media center integration for Linux platforms.
/// Uses MPRIS D-Bus interface.
pub struct MediaCenterInner;

impl MediaCenterInner {
    #[allow(
        clippy::missing_const_for_fn,
        clippy::unnecessary_wraps,
        reason = "MediaCenterInner::new has a cross-platform fallible constructor shape."
    )]
    pub fn new() -> Result<Self, crate::MediaError> {
        Ok(Self)
    }

    #[allow(
        clippy::unused_self,
        reason = "Linux MPRIS media-center updates target the process-global D-Bus state."
    )]
    pub fn update(&self, metadata: &crate::MediaMetadata, state: &crate::PlaybackState) {
        if let Ok(mut guard) = CURRENT_METADATA.write() {
            let mut mpris_metadata = std::collections::HashMap::new();
            mpris_metadata.insert(
                "mpris:trackid".to_string(),
                zbus::zvariant::Value::new(
                    zbus::zvariant::ObjectPath::try_from("/org/waterkit/media/track").unwrap(),
                ),
            );
            if let Some(title) = metadata.title() {
                mpris_metadata.insert(
                    "xesam:title".to_string(),
                    zbus::zvariant::Value::new(title.to_owned()),
                );
            }
            if let Some(artist) = metadata.artist() {
                mpris_metadata.insert(
                    "xesam:artist".to_string(),
                    zbus::zvariant::Value::new(vec![artist.to_owned()]),
                );
            }
            if let Some(album) = metadata.album() {
                mpris_metadata.insert(
                    "xesam:album".to_string(),
                    zbus::zvariant::Value::new(album.to_owned()),
                );
            }
            *guard = mpris_metadata;
        }
        if let Ok(mut guard) = CURRENT_STATUS.write() {
            *guard = state.status();
        }
        if let Some(pos) = state.position()
            && let Ok(mut guard) = CURRENT_POSITION.write()
        {
            match duration_micros_i64(pos) {
                Ok(position) => *guard = position,
                Err(error) => tracing::error!(%error, "failed to update MPRIS position"),
            }
        }
        if let Ok(mut guard) = CURRENT_QUEUE_NAVIGATION.write() {
            *guard = state.queue_navigation_controls();
        }
    }

    #[allow(
        clippy::unused_self,
        reason = "Linux MPRIS media-center updates target the process-global D-Bus state."
    )]
    pub fn clear(&self) {
        if let Ok(mut guard) = CURRENT_METADATA.write() {
            guard.clear();
        }
        if let Ok(mut guard) = CURRENT_STATUS.write() {
            *guard = crate::PlaybackStatus::Stopped;
        }
    }

    #[allow(
        clippy::unused_self,
        reason = "The cross-platform player loop delegates timing to the platform media-center object."
    )]
    pub fn run_loop(&self, duration: std::time::Duration) {
        std::thread::sleep(duration);
    }

    #[allow(
        clippy::missing_const_for_fn,
        clippy::unused_self,
        reason = "Linux player command delivery is owned by the MPRIS session queue."
    )]
    pub fn poll_command(&self) -> Option<crate::MediaCommand> {
        None
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

async fn start_dbus_service() -> Result<Connection, zbus::Error> {
    let connection = ConnectionBuilder::session()?
        .name("org.mpris.MediaPlayer2.waterkit")?
        .serve_at("/org/mpris/MediaPlayer2", MediaPlayer2)?
        .serve_at("/org/mpris/MediaPlayer2", MprisPlayer)?
        .build()
        .await?;

    Ok(connection)
}
