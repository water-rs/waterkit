//! Linux sensor implementation using iio-sensor-proxy D-Bus service.
//!
//! Most Linux desktops don't have motion sensors, but some laptops
//! (like ThinkPads, Surface devices) have accelerometers accessible
//! via the iio-sensor-proxy service.

use crate::{ScalarData, SensorData, SensorError, SensorStream};
use futures::stream;
use zbus::blocking::Connection;

const IIO_PROXY_BUS: &str = "net.hadess.SensorProxy";
const IIO_PROXY_PATH: &str = "/net/hadess/SensorProxy";
const IIO_PROXY_IFACE: &str = "net.hadess.SensorProxy";

fn get_proxy_property<T: for<'a> serde::Deserialize<'a>>(
    conn: &Connection,
    property: &str,
) -> Result<T, SensorError> {
    let proxy = zbus::blocking::fdo::PropertiesProxy::builder(conn)
        .destination(IIO_PROXY_BUS)
        .map_err(|e| SensorError::Unknown(e.to_string()))?
        .path(IIO_PROXY_PATH)
        .map_err(|e| SensorError::Unknown(e.to_string()))?
        .build()
        .map_err(|e| SensorError::Unknown(e.to_string()))?;

    let value = proxy
        .get(IIO_PROXY_IFACE, property)
        .map_err(|e| SensorError::Unknown(e.to_string()))?;

    value
        .downcast_ref::<T>()
        .cloned()
        .ok_or_else(|| SensorError::Unknown("Invalid property type".into()))
}

fn timestamp_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// Accelerometer (via iio-sensor-proxy)
pub fn accelerometer_available() -> bool {
    Connection::system()
        .and_then(|conn| {
            get_proxy_property::<bool>(&conn, "HasAccelerometer")
                .map_err(|_| zbus::Error::Failure("not available".into()))
        })
        .unwrap_or(false)
}

pub async fn accelerometer_read() -> Result<SensorData, SensorError> {
    let conn = Connection::system().map_err(|e| SensorError::Unknown(e.to_string()))?;

    let has = get_proxy_property::<bool>(&conn, "HasAccelerometer")?;
    if !has {
        return Err(SensorError::NotAvailable);
    }

    // iio-sensor-proxy provides orientation as a string, not raw values
    // For actual accelerometer data, we'd need to read from sysfs directly
    // This is a simplified implementation
    let orientation: String = get_proxy_property(&conn, "AccelerometerOrientation")?;

    // Map orientation to approximate accelerometer values
    let (x, y, z) = match orientation.as_str() {
        "normal" => (0.0, 0.0, -1.0),
        "bottom-up" => (0.0, 0.0, 1.0),
        "left-up" => (-1.0, 0.0, 0.0),
        "right-up" => (1.0, 0.0, 0.0),
        _ => (0.0, 0.0, -1.0),
    };

    Ok(SensorData {
        x,
        y,
        z,
        timestamp: timestamp_now(),
    })
}

pub fn accelerometer_watch(interval_ms: u32) -> Result<SensorStream<SensorData>, SensorError> {
    if !accelerometer_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(Box::pin(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        accelerometer_read().await.ok().map(|data| (data, ()))
    })))
}

// Gyroscope (not typically available on Linux laptops)
pub fn gyroscope_available() -> bool {
    false
}

pub async fn gyroscope_read() -> Result<SensorData, SensorError> {
    Err(SensorError::NotAvailable)
}

pub fn gyroscope_watch(_interval_ms: u32) -> Result<SensorStream<SensorData>, SensorError> {
    Err(SensorError::NotAvailable)
}

// Magnetometer (compass via iio-sensor-proxy)
pub fn magnetometer_available() -> bool {
    Connection::system()
        .and_then(|conn| {
            get_proxy_property::<bool>(&conn, "HasCompass")
                .map_err(|_| zbus::Error::Failure("not available".into()))
        })
        .unwrap_or(false)
}

pub async fn magnetometer_read() -> Result<SensorData, SensorError> {
    let conn = Connection::system().map_err(|e| SensorError::Unknown(e.to_string()))?;

    let has = get_proxy_property::<bool>(&conn, "HasCompass")?;
    if !has {
        return Err(SensorError::NotAvailable);
    }

    // Compass heading in degrees
    let heading: f64 = get_proxy_property(&conn, "CompassHeading")?;

    // Convert heading to approximate magnetic field vector
    let rad = heading.to_radians();
    Ok(SensorData {
        x: rad.sin(),
        y: rad.cos(),
        z: 0.0,
        timestamp: timestamp_now(),
    })
}

pub fn magnetometer_watch(interval_ms: u32) -> Result<SensorStream<SensorData>, SensorError> {
    if !magnetometer_available() {
        return Err(SensorError::NotAvailable);
    }
    let interval = std::time::Duration::from_millis(u64::from(interval_ms));
    Ok(Box::pin(stream::unfold((), move |()| async move {
        futures_timer::Delay::new(interval).await;
        magnetometer_read().await.ok().map(|data| (data, ()))
    })))
}

// Barometer (not typically available on Linux laptops)
pub fn barometer_available() -> bool {
    false
}

pub async fn barometer_read() -> Result<ScalarData, SensorError> {
    Err(SensorError::NotAvailable)
}

pub fn barometer_watch(_interval_ms: u32) -> Result<SensorStream<ScalarData>, SensorError> {
    Err(SensorError::NotAvailable)
}
