//! Linux biometric authentication using fprintd via D-Bus.
//!
//! Uses the freedesktop fprintd service for fingerprint authentication.
//! Requires fprintd to be installed and running.

use crate::{BiometricError, BiometricType};
use futures::StreamExt;
use zbus::Connection;

const FPRINTD_BUS_NAME: &str = "net.reactivated.Fprint";
const FPRINTD_MANAGER_PATH: &str = "/net/reactivated/Fprint/Manager";
const FPRINTD_MANAGER_INTERFACE: &str = "net.reactivated.Fprint.Manager";
const FPRINTD_DEVICE_INTERFACE: &str = "net.reactivated.Fprint.Device";

/// Check if fprintd is available and has enrolled fingerprints.
pub async fn is_available() -> bool {
    let Ok(connection) = Connection::system().await else {
        return false;
    };

    // Check if fprintd service exists
    let Ok(dbus_proxy) = zbus::fdo::DBusProxy::new(&connection).await else {
        return false;
    };

    let Ok(names) = dbus_proxy.list_activatable_names().await else {
        return false;
    };

    if !names.iter().any(|n| n.as_str() == FPRINTD_BUS_NAME) {
        return false;
    }

    // Try to get the default device and check for enrolled prints
    match get_default_device(&connection).await {
        Some(device_path) => has_enrolled_prints(&connection, &device_path).await,
        None => false,
    }
}

/// Get the biometric type (always Fingerprint for fprintd).
pub async fn get_biometric_type() -> Option<BiometricType> {
    if is_available().await {
        Some(BiometricType::Fingerprint)
    } else {
        None
    }
}

/// Authenticate using fingerprint.
pub async fn authenticate(reason: &str) -> Result<(), BiometricError> {
    let connection = Connection::system()
        .await
        .map_err(|e| BiometricError::PlatformError(format!("D-Bus connection failed: {e}")))?;

    let device_path = get_default_device(&connection)
        .await
        .ok_or(BiometricError::NotAvailable)?;

    // Get device proxy
    let device_proxy = zbus::Proxy::new(
        &connection,
        FPRINTD_BUS_NAME,
        device_path.as_str(),
        FPRINTD_DEVICE_INTERFACE,
    )
    .await
    .map_err(|e| BiometricError::PlatformError(format!("Device proxy creation failed: {e}")))?;

    // Claim the device for the current user
    let username = get_current_username();
    device_proxy
        .call_method("Claim", &(username.as_str(),))
        .await
        .map_err(|e| BiometricError::PlatformError(format!("Failed to claim device: {e}")))?;

    // Start verification
    let verify_result = device_proxy
        .call_method("VerifyStart", &("any",))
        .await
        .map_err(|e| {
            let _ = device_proxy.call_method::<_, ()>("Release", &());
            BiometricError::PlatformError(format!("Failed to start verification: {e}"))
        });

    if let Err(e) = verify_result {
        return Err(e);
    }

    // Wait for verification result
    // fprintd emits VerifyStatus signal with result
    let result = wait_for_verification(&device_proxy, reason).await;

    // Always release the device
    let _ = device_proxy.call_method::<_, ()>("Release", &()).await;

    result
}

/// Get the default fingerprint device path.
async fn get_default_device(connection: &Connection) -> Option<String> {
    let manager_proxy = zbus::Proxy::new(
        connection,
        FPRINTD_BUS_NAME,
        FPRINTD_MANAGER_PATH,
        FPRINTD_MANAGER_INTERFACE,
    )
    .await
    .ok()?;

    let reply: zbus::zvariant::OwnedObjectPath = manager_proxy
        .call_method("GetDefaultDevice", &())
        .await
        .ok()?
        .body()
        .deserialize()
        .ok()?;

    Some(reply.to_string())
}

/// Check if the device has enrolled fingerprints for the current user.
async fn has_enrolled_prints(connection: &Connection, device_path: &str) -> bool {
    let Ok(device_proxy) = zbus::Proxy::new(
        connection,
        FPRINTD_BUS_NAME,
        device_path,
        FPRINTD_DEVICE_INTERFACE,
    )
    .await
    else {
        return false;
    };

    let username = get_current_username();

    // ListEnrolledFingers returns array of enrolled finger names
    let Ok(reply) = device_proxy
        .call_method("ListEnrolledFingers", &(username.as_str(),))
        .await
    else {
        return false;
    };

    let Ok(fingers): Result<Vec<String>, _> = reply.body().deserialize() else {
        return false;
    };

    !fingers.is_empty()
}

/// Wait for verification result from fprintd.
async fn wait_for_verification(
    device_proxy: &zbus::Proxy<'_>,
    _reason: &str,
) -> Result<(), BiometricError> {
    // fprintd uses signals for verification results
    // We need to listen for VerifyFingerSelected and VerifyStatus signals

    // Create a signal stream for VerifyStatus
    let mut stream = device_proxy
        .receive_signal("VerifyStatus")
        .await
        .map_err(|e| BiometricError::PlatformError(format!("Failed to receive signal: {e}")))?;

    // Wait for the verification result with timeout
    let timeout = tokio::time::Duration::from_secs(30);

    match tokio::time::timeout(timeout, stream.next()).await {
        Ok(Some(signal)) => {
            // Parse the signal body: (result: string, done: bool)
            let body = signal.body();
            let (result, done): (String, bool) = body
                .deserialize()
                .map_err(|e| BiometricError::PlatformError(format!("Signal parse error: {e}")))?;

            if done {
                match result.as_str() {
                    "verify-match" => Ok(()),
                    "verify-no-match" => {
                        Err(BiometricError::Failed("Fingerprint did not match".into()))
                    }
                    "verify-retry-scan" => Err(BiometricError::Failed("Please try again".into())),
                    "verify-swipe-too-short" => {
                        Err(BiometricError::Failed("Swipe was too short".into()))
                    }
                    "verify-finger-not-centered" => {
                        Err(BiometricError::Failed("Finger not centered".into()))
                    }
                    "verify-remove-and-retry" => {
                        Err(BiometricError::Failed("Remove and retry".into()))
                    }
                    "verify-disconnected" => {
                        Err(BiometricError::PlatformError("Device disconnected".into()))
                    }
                    "verify-unknown-error" => Err(BiometricError::Failed("Unknown error".into())),
                    other => Err(BiometricError::Failed(format!(
                        "Verification failed: {other}"
                    ))),
                }
            } else {
                // Not done yet, but for simplicity we'll treat intermediate as retry needed
                Err(BiometricError::Failed("Verification incomplete".into()))
            }
        }
        Ok(None) => Err(BiometricError::Cancelled),
        Err(_) => Err(BiometricError::Failed("Verification timed out".into())),
    }
}

/// Get the current user's username.
fn get_current_username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "root".to_string())
}
