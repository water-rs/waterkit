use crate::{Permission, PermissionError, PermissionStatus};
use js_sys::{Object, Reflect};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::JsFuture;
use web_sys::{MediaStream, MediaStreamConstraints, MediaStreamTrack, PermissionState};

pub async fn check(permission: Permission) -> PermissionStatus {
    let Some(name) = permission_name(permission) else {
        return PermissionStatus::NotDetermined;
    };
    query(name).await.unwrap_or(PermissionStatus::NotDetermined)
}

pub async fn request(permission: Permission) -> Result<PermissionStatus, PermissionError> {
    match permission {
        Permission::Location | Permission::LocationWhenInUse => request_location().await?,
        Permission::Camera => request_media(false).await?,
        Permission::Microphone => request_media(true).await?,
        _ => return Err(PermissionError::Unsupported),
    }
    Ok(PermissionStatus::Granted)
}

async fn query(name: &str) -> Result<PermissionStatus, PermissionError> {
    let descriptor = Object::new();
    Reflect::set(
        &descriptor,
        &JsValue::from_str("name"),
        &JsValue::from_str(name),
    )
    .map_err(platform_error)?;
    let navigator = web_sys::window()
        .ok_or_else(|| PermissionError::Platform(String::from("browser window is unavailable")))?
        .navigator();
    let permissions = navigator.permissions().map_err(platform_error)?;
    let status = JsFuture::from(permissions.query(&descriptor).map_err(platform_error)?)
        .await
        .map_err(platform_error)?
        .dyn_into::<web_sys::PermissionStatus>()
        .map_err(platform_error)?;
    Ok(match status.state() {
        PermissionState::Granted => PermissionStatus::Granted,
        PermissionState::Denied => PermissionStatus::Denied,
        PermissionState::Prompt => PermissionStatus::NotDetermined,
        _ => PermissionStatus::NotDetermined,
    })
}

async fn request_location() -> Result<(), PermissionError> {
    let geolocation = web_sys::window()
        .ok_or_else(|| PermissionError::Platform(String::from("browser window is unavailable")))?
        .navigator()
        .geolocation()
        .map_err(platform_error)?;
    let (sender, receiver) = async_channel::bounded(1);
    let success_sender = sender.clone();
    let success = Closure::<dyn FnMut(JsValue)>::once(move |_| {
        let _ = success_sender.try_send(Ok(()));
    });
    let failure = Closure::<dyn FnMut(JsValue)>::once(move |error| {
        let _ = sender.try_send(Err(platform_error(error)));
    });
    geolocation
        .get_current_position_with_error_callback(
            success.as_ref().unchecked_ref(),
            Some(failure.as_ref().unchecked_ref()),
        )
        .map_err(platform_error)?;
    receiver
        .recv()
        .await
        .map_err(|_| PermissionError::Platform(String::from("geolocation callback closed")))?
}

async fn request_media(audio: bool) -> Result<(), PermissionError> {
    let navigator = web_sys::window()
        .ok_or_else(|| PermissionError::Platform(String::from("browser window is unavailable")))?
        .navigator();
    let constraints = MediaStreamConstraints::new();
    constraints.set_audio_bool(audio);
    constraints.set_video_bool(!audio);
    let stream = JsFuture::from(
        navigator
            .media_devices()
            .map_err(platform_error)?
            .get_user_media_with_constraints(&constraints)
            .map_err(platform_error)?,
    )
    .await
    .map_err(platform_error)?
    .dyn_into::<MediaStream>()
    .map_err(platform_error)?;
    for track in stream.get_tracks() {
        track
            .dyn_into::<MediaStreamTrack>()
            .map_err(platform_error)?
            .stop();
    }
    Ok(())
}

const fn permission_name(permission: Permission) -> Option<&'static str> {
    match permission {
        Permission::Location | Permission::LocationWhenInUse => Some("geolocation"),
        Permission::Camera => Some("camera"),
        Permission::Microphone => Some("microphone"),
        _ => None,
    }
}

fn platform_error(error: JsValue) -> PermissionError {
    PermissionError::Platform(
        error
            .as_string()
            .unwrap_or_else(|| format!("browser permission error: {error:?}")),
    )
}
