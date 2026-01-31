//! Linux media control implementation using MPRIS D-Bus.

// MPRIS interface methods are required by zbus but may not be called from Rust
#![allow(dead_code)]
// These lints apply to D-Bus interface methods and cross-platform API methods
#![allow(
    clippy::unused_self,
    clippy::unnecessary_wraps,
    clippy::missing_const_for_fn,
    clippy::used_underscore_binding, // D-Bus interface parameters may be unused
    clippy::cast_possible_truncation, // MPRIS uses i64 for microseconds
    clippy::cast_sign_loss, // MPRIS position/duration are always positive
    clippy::significant_drop_tightening // Complex lock patterns in D-Bus handlers
)]

use crate::{
    MediaCommand, MediaCommandHandler, MediaError, MediaMetadata, PlaybackState, PlaybackStatus,
};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};
use std::time::Duration;
use zbus::zvariant::{ObjectPath, Value};
use zbus::{Connection, connection::Builder as ConnectionBuilder, interface};

/// Global command handler
static COMMAND_HANDLER: LazyLock<RwLock<Option<Box<dyn MediaCommandHandler>>>> =
    LazyLock::new(|| RwLock::new(None));

/// Current metadata for MPRIS properties
static CURRENT_METADATA: LazyLock<RwLock<HashMap<String, Value<'static>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Current playback status
static CURRENT_STATUS: LazyLock<RwLock<PlaybackStatus>> =
    LazyLock::new(|| RwLock::new(PlaybackStatus::Stopped));

/// Current position in microseconds
static CURRENT_POSITION: LazyLock<RwLock<i64>> = LazyLock::new(|| RwLock::new(0));

/// MPRIS `MediaPlayer2` interface implementation
struct MediaPlayer2;

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
        true
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        true
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

    fn set_position(&self, _track_id: ObjectPath<'_>, position: i64) {
        let duration = Duration::from_micros(position as u64);
        dispatch_command(MediaCommand::Seek(duration));
    }

    fn open_uri(&self, _uri: String) {
        // Not implemented
    }
}

fn dispatch_command(cmd: MediaCommand) {
    if let Ok(guard) = COMMAND_HANDLER.read()
        && let Some(handler) = guard.as_ref()
    {
        handler.on_command(cmd);
    }
}

#[derive(Debug)]
pub struct MediaSessionInner {
    connection: Arc<RwLock<Option<Connection>>>,
}

impl MediaSessionInner {
    pub fn new() -> Result<Self, MediaError> {
        // Start the D-Bus service in a background thread
        let connection = Arc::new(RwLock::new(None));
        let conn_clone = Arc::clone(&connection);

        std::thread::spawn(move || {
            smol::block_on(async {
                match start_dbus_service().await {
                    Ok(conn) => {
                        if let Ok(mut guard) = conn_clone.write() {
                            *guard = Some(conn);
                        }
                        // Keep the connection alive
                        std::future::pending::<()>().await;
                    }
                    Err(e) => {
                        eprintln!("Failed to start MPRIS service: {e}");
                    }
                }
            });
        });

        Ok(Self { connection })
    }

    pub fn set_metadata(&self, metadata: &MediaMetadata) -> Result<(), MediaError> {
        let mut mpris_metadata: HashMap<String, Value<'static>> = HashMap::new();

        // Track ID is required
        mpris_metadata.insert(
            "mpris:trackid".to_string(),
            Value::new(ObjectPath::try_from("/org/waterkit/media/track").unwrap()),
        );

        if let Some(ref title) = metadata.title {
            mpris_metadata.insert("xesam:title".to_string(), Value::new(title.clone()));
        }

        if let Some(ref artist) = metadata.artist {
            mpris_metadata.insert("xesam:artist".to_string(), Value::new(vec![artist.clone()]));
        }

        if let Some(ref album) = metadata.album {
            mpris_metadata.insert("xesam:album".to_string(), Value::new(album.clone()));
        }

        if let Some(ref url) = metadata.artwork_url {
            mpris_metadata.insert("mpris:artUrl".to_string(), Value::new(url.clone()));
        }

        if let Some(duration) = metadata.duration {
            mpris_metadata.insert(
                "mpris:length".to_string(),
                Value::new(duration.as_micros() as i64),
            );
        }

        if let Ok(mut guard) = CURRENT_METADATA.write() {
            *guard = mpris_metadata;
        }

        Ok(())
    }

    pub fn set_playback_state(&self, state: &PlaybackState) -> Result<(), MediaError> {
        if let Ok(mut guard) = CURRENT_STATUS.write() {
            *guard = state.status;
        }

        if let Some(pos) = state.position
            && let Ok(mut guard) = CURRENT_POSITION.write()
        {
            *guard = pos.as_micros() as i64;
        }

        Ok(())
    }

    pub fn set_command_handler(
        &self,
        handler: Box<dyn MediaCommandHandler>,
    ) -> Result<(), MediaError> {
        let mut guard = COMMAND_HANDLER
            .write()
            .map_err(|e| MediaError::Unknown(format!("Lock poisoned: {e}")))?;
        *guard = Some(handler);
        Ok(())
    }

    pub fn request_audio_focus(&self) -> Result<(), MediaError> {
        // Linux doesn't have a centralized audio focus system
        Ok(())
    }

    pub fn abandon_audio_focus(&self) -> Result<(), MediaError> {
        Ok(())
    }

    pub fn clear(&self) -> Result<(), MediaError> {
        if let Ok(mut guard) = CURRENT_METADATA.write() {
            guard.clear();
        }
        if let Ok(mut guard) = CURRENT_STATUS.write() {
            *guard = PlaybackStatus::Stopped;
        }
        Ok(())
    }
}

/// Media center integration for Linux platforms.
/// Uses MPRIS D-Bus interface.
pub struct MediaCenterInner;

impl MediaCenterInner {
    pub fn new() -> Result<Self, crate::MediaError> {
        Ok(Self)
    }

    #[allow(clippy::unused_self)]
    pub fn update(&self, metadata: &crate::MediaMetadata, state: &crate::PlaybackState) {
        if let Ok(mut guard) = CURRENT_METADATA.write() {
            let mut mpris_metadata = std::collections::HashMap::new();
            mpris_metadata.insert(
                "mpris:trackid".to_string(),
                zbus::zvariant::Value::new(
                    zbus::zvariant::ObjectPath::try_from("/org/waterkit/media/track").unwrap(),
                ),
            );
            if let Some(ref title) = metadata.title {
                mpris_metadata.insert(
                    "xesam:title".to_string(),
                    zbus::zvariant::Value::new(title.clone()),
                );
            }
            if let Some(ref artist) = metadata.artist {
                mpris_metadata.insert(
                    "xesam:artist".to_string(),
                    zbus::zvariant::Value::new(vec![artist.clone()]),
                );
            }
            if let Some(ref album) = metadata.album {
                mpris_metadata.insert(
                    "xesam:album".to_string(),
                    zbus::zvariant::Value::new(album.clone()),
                );
            }
            *guard = mpris_metadata;
        }
        if let Ok(mut guard) = CURRENT_STATUS.write() {
            *guard = state.status;
        }
        if let Some(pos) = state.position
            && let Ok(mut guard) = CURRENT_POSITION.write()
        {
            *guard = pos.as_micros() as i64;
        }
    }

    #[allow(clippy::unused_self)]
    pub fn clear(&self) {
        if let Ok(mut guard) = CURRENT_METADATA.write() {
            guard.clear();
        }
        if let Ok(mut guard) = CURRENT_STATUS.write() {
            *guard = crate::PlaybackStatus::Stopped;
        }
    }

    #[allow(clippy::unused_self)]
    pub fn run_loop(&self, duration: std::time::Duration) {
        std::thread::sleep(duration);
    }

    #[allow(clippy::unused_self)]
    pub fn poll_command(&self) -> Option<crate::MediaCommand> {
        None
    }
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
