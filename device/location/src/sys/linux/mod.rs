//! Linux location implementation using `GeoClue2` D-Bus service.

use core::time::Duration;

use crate::{Location, LocationError, Timestamp};
use futures::StreamExt;
use futures::future::{Either, select};
use zbus::Connection;
use zbus::zvariant::OwnedObjectPath;

/// How long `GeoClue2` may take to produce the first fix before the request
/// fails with [`LocationError::Timeout`]. Matches the Apple implementation's
/// `locationRequestTimeout`.
const LOCATION_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

const GEOCLUE_SERVICE: &str = "org.freedesktop.GeoClue2";
const CLIENT_INTERFACE: &str = "org.freedesktop.GeoClue2.Client";
const LOCATION_INTERFACE: &str = "org.freedesktop.GeoClue2.Location";

/// `GeoClue2` reports an unknown altitude as `f64::MIN`.
fn altitude_is_known(altitude: f64) -> bool {
    altitude > f64::MIN / 2.0
}

async fn location_property<T>(
    connection: &Connection,
    location_path: &str,
    prop: &str,
) -> Result<T, LocationError>
where
    T: TryFrom<zbus::zvariant::OwnedValue>,
    T::Error: std::fmt::Display,
{
    let reply: zbus::zvariant::OwnedValue = connection
        .call_method(
            Some(GEOCLUE_SERVICE),
            location_path,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &(LOCATION_INTERFACE, prop),
        )
        .await
        .map_err(|e| LocationError::Platform(format!("Failed to get {prop}: {e}")))?
        .body()
        .deserialize()
        .map_err(|e| LocationError::Platform(format!("Failed to parse {prop}: {e}")))?;

    T::try_from(reply).map_err(|e| {
        LocationError::Platform(format!("GeoClue2 {prop} had an unexpected type: {e}"))
    })
}

/// Reads the client's current `Location` property; `/` means no fix yet.
async fn current_location_path(
    connection: &Connection,
    client_path: &str,
) -> Result<Option<OwnedObjectPath>, LocationError> {
    let reply: zbus::zvariant::OwnedValue = connection
        .call_method(
            Some(GEOCLUE_SERVICE),
            client_path,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &(CLIENT_INTERFACE, "Location"),
        )
        .await
        .map_err(|e| LocationError::Platform(format!("Failed to get location: {e}")))?
        .body()
        .deserialize()
        .map_err(|e| LocationError::Platform(format!("Failed to parse location path: {e}")))?;

    let path: OwnedObjectPath = reply
        .downcast_ref::<zbus::zvariant::ObjectPath>()
        .map(|p| p.to_owned().into())
        .map_err(|e| {
            LocationError::Platform(format!("GeoClue2 Location was not an object path: {e}"))
        })?;
    if path.as_str() == "/" {
        Ok(None)
    } else {
        Ok(Some(path))
    }
}

async fn read_location(
    connection: &Connection,
    location_path: &str,
) -> Result<Location, LocationError> {
    let latitude: f64 = location_property(connection, location_path, "Latitude").await?;
    let longitude: f64 = location_property(connection, location_path, "Longitude").await?;
    let accuracy: f64 = location_property(connection, location_path, "Accuracy").await?;
    let altitude: f64 = location_property(connection, location_path, "Altitude").await?;

    // GeoClue2 reports when the fix was taken as (seconds, microseconds) since
    // the Unix epoch; for navigation the fix's age matters, so keep it instead
    // of stamping "now".
    let (seconds, microseconds): (u64, u64) =
        location_property(connection, location_path, "Timestamp").await?;
    let nanoseconds = i128::from(seconds) * 1_000_000_000 + i128::from(microseconds) * 1_000;
    let timestamp = Timestamp::from_nanosecond(nanoseconds)
        .map_err(|e| LocationError::Platform(format!("GeoClue2 timestamp invalid: {e}")))?;

    let mut location =
        Location::from_degrees(latitude, longitude, timestamp)?.with_horizontal_accuracy(accuracy);
    if altitude_is_known(altitude) {
        location = location.with_altitude(altitude);
    }
    Ok(location)
}

pub async fn get_location() -> Result<Location, LocationError> {
    let connection = Connection::system()
        .await
        .map_err(|e| LocationError::Platform(format!("D-Bus connection failed: {e}")))?;

    let reply: (OwnedObjectPath,) = connection
        .call_method(
            Some(GEOCLUE_SERVICE),
            "/org/freedesktop/GeoClue2/Manager",
            Some("org.freedesktop.GeoClue2.Manager"),
            "GetClient",
            &(),
        )
        .await
        .map_err(|e| LocationError::Platform(format!("GeoClue2 not available: {e}")))?
        .body()
        .deserialize()
        .map_err(|e| LocationError::Platform(format!("Failed to parse response: {e}")))?;
    let client_path = reply.0;

    connection
        .call_method(
            Some(GEOCLUE_SERVICE),
            client_path.as_str(),
            Some("org.freedesktop.DBus.Properties"),
            "Set",
            &(
                CLIENT_INTERFACE,
                "DesktopId",
                zbus::zvariant::Value::from("waterkit"),
            ),
        )
        .await
        .map_err(|e| LocationError::Platform(format!("Failed to set desktop ID: {e}")))?;

    // Subscribe before Start so the first LocationUpdated cannot be missed.
    let client_proxy = zbus::Proxy::new(
        &connection,
        GEOCLUE_SERVICE,
        client_path.as_str(),
        CLIENT_INTERFACE,
    )
    .await
    .map_err(|e| LocationError::Platform(format!("Failed to create client proxy: {e}")))?;
    let mut updates = client_proxy
        .receive_signal("LocationUpdated")
        .await
        .map_err(|e| LocationError::Platform(format!("Failed to subscribe to updates: {e}")))?;

    client_proxy
        .call_method("Start", &())
        .await
        .map_err(|e| LocationError::Platform(format!("Failed to start GeoClue client: {e}")))?;

    // The property is only valid once GeoClue has a fix; until then it reads
    // `/` and the fix arrives through LocationUpdated.
    let outcome = async {
        let location_path = match current_location_path(&connection, client_path.as_str()).await? {
            Some(path) => path,
            None => {
                match select(
                    updates.next(),
                    Box::pin(async_io::Timer::after(LOCATION_REQUEST_TIMEOUT)),
                )
                .await
                {
                    Either::Left((Some(signal), _)) => {
                        let (_old, new): (OwnedObjectPath, OwnedObjectPath) =
                            signal.body().deserialize().map_err(|e| {
                                LocationError::Platform(format!("Failed to parse update: {e}"))
                            })?;
                        new
                    }
                    Either::Left((None, _)) => {
                        return Err(LocationError::Platform(String::from(
                            "GeoClue2 signal stream ended before a fix arrived",
                        )));
                    }
                    Either::Right(_) => return Err(LocationError::Timeout),
                }
            }
        };
        read_location(&connection, location_path.as_str()).await
    }
    .await;

    // Stop the client regardless of the outcome so GeoClue releases the fix.
    let _ = client_proxy.call_method("Stop", &()).await;

    outcome
}
