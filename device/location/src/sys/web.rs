use crate::{Location, LocationError};
use jiff::{SignedDuration, Timestamp};
use js_sys::Reflect;
use wasm_bindgen::{JsCast, JsValue, closure::Closure, prelude::wasm_bindgen};

// `web-sys` binds `GeolocationPositionError` only behind
// `--cfg web_sys_unstable_apis`, so the two members read here are bound
// directly.
#[wasm_bindgen]
extern "C" {
    type GeolocationPositionError;

    #[wasm_bindgen(method, getter)]
    fn code(this: &GeolocationPositionError) -> u16;

    #[wasm_bindgen(method, getter)]
    fn message(this: &GeolocationPositionError) -> String;
}

// The `GeolocationPositionError` codes.
const PERMISSION_DENIED: u16 = 1;
const POSITION_UNAVAILABLE: u16 = 2;
const TIMEOUT: u16 = 3;

#[expect(
    clippy::future_not_send,
    reason = "holds the JS success and error callbacks across the await, and a `Closure` is bound to the thread that created it"
)]
pub async fn get_location() -> Result<Location, LocationError> {
    let geolocation = web_sys::window()
        .ok_or_else(|| LocationError::Platform(String::from("browser window is unavailable")))?
        .navigator()
        .geolocation()
        .map_err(|error| platform_error(&error))?;
    let (sender, receiver) = async_channel::bounded(1);
    let success_sender = sender.clone();
    let success = Closure::<dyn FnMut(JsValue)>::once(move |position| {
        let _ = success_sender.try_send(location_from_position(&position));
    });
    let failure = Closure::<dyn FnMut(JsValue)>::once(move |error| {
        let _ = sender.try_send(Err(geolocation_error(&error)));
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
        .map_err(|_| LocationError::Platform(String::from("geolocation callback closed")))?
}

fn location_from_position(position: &JsValue) -> Result<Location, LocationError> {
    let coords = Reflect::get(position, &JsValue::from_str("coords"))
        .map_err(|error| platform_error(&error))?;
    let latitude = number_property(&coords, "latitude")?;
    let longitude = number_property(&coords, "longitude")?;
    // `timestamp` is an `EpochTimeStamp`: milliseconds since the Unix epoch.
    let timestamp_millis = number_property(position, "timestamp")?;
    let timestamp = SignedDuration::try_from_secs_f64(timestamp_millis / 1000.0)
        .and_then(Timestamp::from_duration)
        .map_err(|error| LocationError::Platform(error.to_string()))?;
    let mut location = Location::from_degrees(latitude, longitude, timestamp)?;
    if let Some(altitude) = optional_number_property(&coords, "altitude")? {
        location = location.with_altitude(altitude);
    }
    if let Some(accuracy) = optional_number_property(&coords, "accuracy")? {
        location = location.with_horizontal_accuracy(accuracy);
    }
    if let Some(accuracy) = optional_number_property(&coords, "altitudeAccuracy")? {
        location = location.with_vertical_accuracy(accuracy);
    }
    Ok(location)
}

fn number_property(value: &JsValue, name: &str) -> Result<f64, LocationError> {
    Reflect::get(value, &JsValue::from_str(name))
        .map_err(|error| platform_error(&error))?
        .as_f64()
        .ok_or_else(|| LocationError::Platform(format!("geolocation {name} is not a number")))
}

fn optional_number_property(value: &JsValue, name: &str) -> Result<Option<f64>, LocationError> {
    let value =
        Reflect::get(value, &JsValue::from_str(name)).map_err(|error| platform_error(&error))?;
    if value.is_null() || value.is_undefined() {
        Ok(None)
    } else {
        value
            .as_f64()
            .map(Some)
            .ok_or_else(|| LocationError::Platform(format!("geolocation {name} is not a number")))
    }
}

fn geolocation_error(error: &JsValue) -> LocationError {
    let Some(error) = error.dyn_ref::<GeolocationPositionError>() else {
        return platform_error(error);
    };
    match error.code() {
        PERMISSION_DENIED => LocationError::PermissionDenied,
        POSITION_UNAVAILABLE => LocationError::NotAvailable,
        TIMEOUT => LocationError::Timeout,
        code => LocationError::Platform(format!(
            "browser geolocation error {code}: {}",
            error.message()
        )),
    }
}

fn platform_error(error: &JsValue) -> LocationError {
    LocationError::Platform(
        error
            .as_string()
            .unwrap_or_else(|| format!("browser geolocation error: {error:?}")),
    )
}
