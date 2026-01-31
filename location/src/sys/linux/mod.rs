//! Linux location implementation using `GeoClue2` D-Bus service.

use crate::{Location, LocationError, Timestamp};
use zbus::Connection;

async fn get_location_property(
    connection: &Connection,
    location_path: &str,
    prop: &str,
) -> Result<f64, zbus::Error> {
    let reply: zbus::zvariant::OwnedValue = connection
        .call_method(
            Some("org.freedesktop.GeoClue2"),
            location_path,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.freedesktop.GeoClue2.Location", prop),
        )
        .await?
        .body()
        .deserialize()?;
    Ok(f64::try_from(reply).unwrap_or_default())
}

pub async fn get_location() -> Result<Location, LocationError> {
    // Connect to the system bus
    let connection = Connection::system()
        .await
        .map_err(|e| LocationError::Unknown(format!("D-Bus connection failed: {e}")))?;

    // Call GeoClue2 Manager to get a client
    let reply: (zbus::zvariant::OwnedObjectPath,) = connection
        .call_method(
            Some("org.freedesktop.GeoClue2"),
            "/org/freedesktop/GeoClue2/Manager",
            Some("org.freedesktop.GeoClue2.Manager"),
            "GetClient",
            &(),
        )
        .await
        .map_err(|e| LocationError::Unknown(format!("GeoClue2 not available: {e}")))?
        .body()
        .deserialize()
        .map_err(|e| LocationError::Unknown(format!("Failed to parse response: {e}")))?;

    let client_path = reply.0;

    // Set the desktop ID (required by GeoClue2)
    connection
        .call_method(
            Some("org.freedesktop.GeoClue2"),
            client_path.as_str(),
            Some("org.freedesktop.DBus.Properties"),
            "Set",
            &(
                "org.freedesktop.GeoClue2.Client",
                "DesktopId",
                zbus::zvariant::Value::from("waterkit"),
            ),
        )
        .await
        .map_err(|e| LocationError::Unknown(format!("Failed to set desktop ID: {e}")))?;

    // Start the client
    connection
        .call_method(
            Some("org.freedesktop.GeoClue2"),
            client_path.as_str(),
            Some("org.freedesktop.GeoClue2.Client"),
            "Start",
            &(),
        )
        .await
        .map_err(|e| LocationError::Unknown(format!("Failed to start GeoClue client: {e}")))?;

    // Get the location object path
    let location_reply: zbus::zvariant::OwnedValue = connection
        .call_method(
            Some("org.freedesktop.GeoClue2"),
            client_path.as_str(),
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.freedesktop.GeoClue2.Client", "Location"),
        )
        .await
        .map_err(|e| LocationError::Unknown(format!("Failed to get location: {e}")))?
        .body()
        .deserialize()
        .map_err(|e| LocationError::Unknown(format!("Failed to parse location path: {e}")))?;

    let location_path: zbus::zvariant::OwnedObjectPath = location_reply
        .downcast_ref::<zbus::zvariant::ObjectPath>()
        .map(|p| p.to_owned().into())
        .map_err(|_| LocationError::NotAvailable)?;

    // Get latitude and longitude from the location object
    let latitude = get_location_property(&connection, location_path.as_str(), "Latitude")
        .await
        .map_err(|e| LocationError::Unknown(format!("Failed to get latitude: {e}")))?;
    let longitude = get_location_property(&connection, location_path.as_str(), "Longitude")
        .await
        .map_err(|e| LocationError::Unknown(format!("Failed to get longitude: {e}")))?;
    let altitude = get_location_property(&connection, location_path.as_str(), "Altitude")
        .await
        .ok();
    let accuracy = get_location_property(&connection, location_path.as_str(), "Accuracy")
        .await
        .ok();

    // Stop the client
    let _ = connection
        .call_method(
            Some("org.freedesktop.GeoClue2"),
            client_path.as_str(),
            Some("org.freedesktop.GeoClue2.Client"),
            "Stop",
            &(),
        )
        .await;

    let timestamp = Timestamp::now();

    let mut location = Location::new(latitude, longitude, timestamp);

    if let Some(alt) = altitude {
        location = location.with_altitude(alt);
    }
    if let Some(acc) = accuracy {
        location = location.with_horizontal_accuracy(acc);
    }

    Ok(location)
}
