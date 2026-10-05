use crate::{Permission, PermissionError, PermissionStatus};
use js_sys::{Object, Reflect};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::JsFuture;
use web_sys::{MediaStream, MediaStreamConstraints, MediaStreamTrack, PermissionState};

#[expect(
    clippy::future_not_send,
    reason = "awaits `query`, which is bound to the browser's thread"
)]
pub async fn check(permission: Permission) -> PermissionStatus {
    let Some(name) = permission_name(permission) else {
        return PermissionStatus::NotDetermined;
    };
    query(name).await.unwrap_or(PermissionStatus::NotDetermined)
}

#[expect(
    clippy::future_not_send,
    reason = "awaits the browser prompts, which are bound to the browser's thread"
)]
pub async fn request(permission: Permission) -> Result<PermissionStatus, PermissionError> {
    match permission {
        Permission::Location | Permission::LocationWhenInUse => request_location().await?,
        Permission::Camera => request_media(false).await?,
        Permission::Microphone => request_media(true).await?,
        _ => return Err(PermissionError::Unsupported),
    }
    Ok(PermissionStatus::Granted)
}

#[expect(
    clippy::future_not_send,
    reason = "awaits a `JsFuture`, whose shared state is an `Rc<RefCell<_>>` holding JS callbacks"
)]
async fn query(name: &str) -> Result<PermissionStatus, PermissionError> {
    let descriptor = Object::new();
    Reflect::set(
        &descriptor,
        &JsValue::from_str("name"),
        &JsValue::from_str(name),
    )
    .map_err(|error| platform_error(&error))?;
    let navigator = web_sys::window()
        .ok_or_else(|| PermissionError::Platform(String::from("browser window is unavailable")))?
        .navigator();
    let permissions = navigator
        .permissions()
        .map_err(|error| platform_error(&error))?;
    let status = JsFuture::from(
        permissions
            .query(&descriptor)
            .map_err(|error| platform_error(&error))?,
    )
    .await
    .map_err(|error| platform_error(&error))?
    .dyn_into::<web_sys::PermissionStatus>()
    .map_err(|error| platform_error(&error))?;
    match status.state() {
        PermissionState::Granted => Ok(PermissionStatus::Granted),
        PermissionState::Denied => Ok(PermissionStatus::Denied),
        PermissionState::Prompt => Ok(PermissionStatus::NotDetermined),
        state => Err(PermissionError::Platform(format!(
            "browser reported unknown permission state {state:?}"
        ))),
    }
}

#[expect(
    clippy::future_not_send,
    reason = "holds the JS success and error callbacks across the await, and a `Closure` is bound to the thread that created it"
)]
async fn request_location() -> Result<(), PermissionError> {
    let geolocation = web_sys::window()
        .ok_or_else(|| PermissionError::Platform(String::from("browser window is unavailable")))?
        .navigator()
        .geolocation()
        .map_err(|error| platform_error(&error))?;
    let (sender, receiver) = async_channel::bounded(1);
    let success_sender = sender.clone();
    let success = Closure::<dyn FnMut(JsValue)>::once(move |_| {
        let _ = success_sender.try_send(Ok(()));
    });
    let failure = Closure::<dyn FnMut(JsValue)>::once(move |error| {
        let _ = sender.try_send(Err(platform_error(&error)));
    });
    geolocation
        .get_current_position_with_error_callback(
            success.as_ref().unchecked_ref(),
            Some(failure.as_ref().unchecked_ref()),
        )
        .map_err(|error| platform_error(&error))?;
    receiver
        .recv()
        .await
        .map_err(|_| PermissionError::Platform(String::from("geolocation callback closed")))?
}

#[expect(
    clippy::future_not_send,
    reason = "awaits a `JsFuture`, whose shared state is an `Rc<RefCell<_>>` holding JS callbacks"
)]
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
            .map_err(|error| platform_error(&error))?
            .get_user_media_with_constraints(&constraints)
            .map_err(|error| platform_error(&error))?,
    )
    .await
    .map_err(|error| platform_error(&error))?
    .dyn_into::<MediaStream>()
    .map_err(|error| platform_error(&error))?;
    for track in stream.get_tracks() {
        track
            .dyn_into::<MediaStreamTrack>()
            .map_err(|error| platform_error(&error))?
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

fn platform_error(error: &JsValue) -> PermissionError {
    PermissionError::Platform(
        error
            .as_string()
            .unwrap_or_else(|| format!("browser permission error: {error:?}")),
    )
}
