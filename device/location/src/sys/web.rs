use crate::{Location, LocationError};
use js_sys::{Date, Reflect};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};

pub async fn get_location() -> Result<Location, LocationError> {
    let geolocation = web_sys::window()
        .ok_or_else(|| LocationError::Platform(String::from("browser window is unavailable")))?
        .navigator()
        .geolocation()
        .map_err(platform_error)?;
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
        .map_err(platform_error)?;
    receiver
        .recv()
        .await
        .map_err(|_| LocationError::Platform(String::from("geolocation callback closed")))?
}

fn location_from_position(position: &JsValue) -> Result<Location, LocationError> {
    let coords = Reflect::get(position, &JsValue::from_str("coords")).map_err(platform_error)?;
    let latitude = number_property(&coords, "latitude")?;
    let longitude = number_property(&coords, "longitude")?;
    let timestamp_millis = Reflect::get(position, &JsValue::from_str("timestamp"))
        .map_err(platform_error)?
        .as_f64()
        .unwrap_or_else(Date::now) as i64;
    let timestamp = jiff::Timestamp::from_millisecond(timestamp_millis)
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
        .map_err(platform_error)?
        .as_f64()
        .ok_or_else(|| LocationError::Platform(format!("geolocation {name} is not a number")))
}

fn optional_number_property(value: &JsValue, name: &str) -> Result<Option<f64>, LocationError> {
    let value = Reflect::get(value, &JsValue::from_str(name)).map_err(platform_error)?;
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
    match Reflect::get(error, &JsValue::from_str("code"))
        .ok()
        .and_then(|value| value.as_f64())
        .map(|code| code as u16)
    {
        Some(1) => LocationError::PermissionDenied,
        Some(2) => LocationError::NotAvailable,
        Some(3) => LocationError::Timeout,
        _ => platform_error(error.clone()),
    }
}

fn platform_error(error: JsValue) -> LocationError {
    LocationError::Platform(
        error
            .as_string()
            .unwrap_or_else(|| format!("browser geolocation error: {error:?}")),
    )
}
