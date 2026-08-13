use crate::{MediaCommand, MediaError, MediaMetadata, PlaybackState, PlaybackStatus};
use js_sys::{Function, Object, Reflect};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};

pub struct MediaSessionInner {
    session: Object,
    command_receiver: async_channel::Receiver<MediaCommand>,
    handlers: Vec<Closure<dyn FnMut(JsValue)>>,
}

impl std::fmt::Debug for MediaSessionInner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MediaSessionInner")
            .finish_non_exhaustive()
    }
}

impl MediaSessionInner {
    pub fn new() -> Result<Self, MediaError> {
        let navigator = web_sys::window()
            .ok_or_else(|| {
                MediaError::InitializationFailed(String::from("browser window is unavailable"))
            })?
            .navigator();
        let session = Reflect::get(&navigator, &JsValue::from_str("mediaSession"))
            .map_err(initialization_error)?
            .dyn_into::<Object>()
            .map_err(initialization_error)?;
        let (sender, command_receiver) = async_channel::unbounded();
        let mut handlers = Vec::new();
        for (action, command) in [
            ("play", MediaCommand::Play),
            ("pause", MediaCommand::Pause),
            ("stop", MediaCommand::Stop),
            ("nexttrack", MediaCommand::Next),
            ("previoustrack", MediaCommand::Previous),
        ] {
            handlers.push(install_handler(&session, action, command, &sender)?);
        }
        Ok(Self {
            session,
            command_receiver,
            handlers,
        })
    }

    pub fn set_metadata(&self, metadata: &MediaMetadata) -> Result<(), MediaError> {
        let init = Object::new();
        set_string(&init, "title", metadata.title().unwrap_or_default())?;
        set_string(&init, "artist", metadata.artist().unwrap_or_default())?;
        set_string(&init, "album", metadata.album().unwrap_or_default())?;
        let constructor = Reflect::get(&js_sys::global(), &JsValue::from_str("MediaMetadata"))
            .map_err(update_error)?
            .dyn_into::<Function>()
            .map_err(update_error)?;
        let browser_metadata =
            Reflect::construct(&constructor, &js_sys::Array::of1(&init)).map_err(update_error)?;
        Reflect::set(
            &self.session,
            &JsValue::from_str("metadata"),
            &browser_metadata,
        )
        .map_err(update_error)?;
        Ok(())
    }

    pub fn set_playback_state(&self, state: &PlaybackState) -> Result<(), MediaError> {
        let status = match state.status() {
            PlaybackStatus::Playing => "playing",
            PlaybackStatus::Paused => "paused",
            PlaybackStatus::Stopped => "none",
        };
        Reflect::set(
            &self.session,
            &JsValue::from_str("playbackState"),
            &JsValue::from_str(status),
        )
        .map_err(update_error)?;
        Ok(())
    }

    pub const fn request_audio_focus(&self) -> Result<(), MediaError> {
        Ok(())
    }

    pub const fn abandon_audio_focus(&self) -> Result<(), MediaError> {
        Ok(())
    }

    pub fn clear(&self) -> Result<(), MediaError> {
        Reflect::set(
            &self.session,
            &JsValue::from_str("metadata"),
            &JsValue::NULL,
        )
        .map_err(update_error)?;
        Reflect::set(
            &self.session,
            &JsValue::from_str("playbackState"),
            &JsValue::from_str("none"),
        )
        .map_err(update_error)?;
        Ok(())
    }

    pub fn command_receiver(&self) -> async_channel::Receiver<MediaCommand> {
        self.command_receiver.clone()
    }
}

impl Drop for MediaSessionInner {
    fn drop(&mut self) {
        let _ = self.clear();
        let set_action_handler =
            Reflect::get(&self.session, &JsValue::from_str("setActionHandler"))
                .expect("browser MediaSession must keep setActionHandler available")
                .dyn_into::<Function>()
                .expect("MediaSession.setActionHandler must be a function");
        for action in ["play", "pause", "stop", "nexttrack", "previoustrack"] {
            let _ =
                set_action_handler.call2(&self.session, &JsValue::from_str(action), &JsValue::NULL);
        }
        self.handlers.clear();
    }
}

fn install_handler(
    session: &Object,
    action: &'static str,
    command: MediaCommand,
    sender: &async_channel::Sender<MediaCommand>,
) -> Result<Closure<dyn FnMut(JsValue)>, MediaError> {
    let sender = sender.clone();
    let handler = Closure::wrap(Box::new(move |_| {
        let _ = sender.try_send(command.clone());
    }) as Box<dyn FnMut(JsValue)>);
    Reflect::get(session, &JsValue::from_str("setActionHandler"))
        .map_err(initialization_error)?
        .dyn_into::<Function>()
        .map_err(initialization_error)?
        .call2(session, &JsValue::from_str(action), handler.as_ref())
        .map_err(initialization_error)?;
    Ok(handler)
}

fn set_string(object: &Object, name: &str, value: &str) -> Result<(), MediaError> {
    Reflect::set(object, &JsValue::from_str(name), &JsValue::from_str(value))
        .map(|_| ())
        .map_err(update_error)
}

fn initialization_error(error: JsValue) -> MediaError {
    MediaError::InitializationFailed(js_error(error))
}

fn update_error(error: JsValue) -> MediaError {
    MediaError::UpdateFailed(js_error(error))
}

fn js_error(error: JsValue) -> String {
    error
        .as_string()
        .unwrap_or_else(|| format!("browser media session error: {error:?}"))
}
