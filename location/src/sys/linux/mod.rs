//! Linux location implementation using GeoClue2 D-Bus service.

use crate::{Location, LocationError};

pub(crate) async fn get_location() -> Result<Location, LocationError> {
    use zbus::Connection;

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
        .ok_or_else(|| LocationError::NotAvailable)?;

    // Get latitude and longitude from the location object
    let get_property = |prop: &str| async {
        let reply: zbus::zvariant::OwnedValue = connection
            .call_method(
                Some("org.freedesktop.GeoClue2"),
                location_path.as_str(),
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &("org.freedesktop.GeoClue2.Location", prop),
            )
            .await?
            .body()
            .deserialize()?;
        Ok::<f64, zbus::Error>(reply.downcast_ref::<f64>().copied().unwrap_or(0.0))
    };

    let latitude = get_property("Latitude")
        .await
        .map_err(|e| LocationError::Unknown(format!("Failed to get latitude: {e}")))?;
    let longitude = get_property("Longitude")
        .await
        .map_err(|e| LocationError::Unknown(format!("Failed to get longitude: {e}")))?;
    let altitude = get_property("Altitude").await.ok();
    let accuracy = get_property("Accuracy").await.ok();

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

    Ok(Location {
        latitude,
        longitude,
        altitude,
        horizontal_accuracy: accuracy,
        vertical_accuracy: None,
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
    })
}
