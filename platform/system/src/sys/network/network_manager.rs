//! Connectivity as `NetworkManager` reports it.
//!
//! `NetworkManager`'s `State` already folds in its connectivity check: it is
//! `NM_STATE_CONNECTED_GLOBAL` only when the default route reaches the
//! internet (or, with the check disabled, whenever an active connection holds
//! the default route). `PrimaryConnectionType` names the connection that holds
//! that route.

use crate::{ConnectionType, SystemError};

/// `NMState` values, from `NetworkManager`'s `nm-dbus-interface.h`.
mod state {
    pub const UNKNOWN: u32 = 0;
    pub const ASLEEP: u32 = 10;
    pub const DISCONNECTED: u32 = 20;
    pub const DISCONNECTING: u32 = 30;
    pub const CONNECTING: u32 = 40;
    pub const CONNECTED_LOCAL: u32 = 50;
    pub const CONNECTED_SITE: u32 = 60;
    pub const CONNECTED_GLOBAL: u32 = 70;
}

/// The transport of the internet connection, from `NetworkManager`'s `State`
/// and `PrimaryConnectionType` properties, or [`ConnectionType::None`] when
/// `NetworkManager` reports no internet connection.
///
/// # Errors
///
/// [`SystemError::Platform`] when `NetworkManager` reports its state as unknown,
/// which it documents as a daemon error, reports a state this crate does not
/// know, or reports a global connection without a primary connection.
pub fn transport(state: u32, primary_connection_type: &str) -> Result<ConnectionType, SystemError> {
    match state {
        state::CONNECTED_GLOBAL => {}
        // Asleep, disconnected, or connected without a route to the internet:
        // a link-local network, or one behind a captive portal.
        state::ASLEEP
        | state::DISCONNECTED
        | state::DISCONNECTING
        | state::CONNECTING
        | state::CONNECTED_LOCAL
        | state::CONNECTED_SITE => return Ok(ConnectionType::None),
        state::UNKNOWN => {
            return Err(SystemError::Platform(String::from(
                "NetworkManager reports its networking state as unknown",
            )));
        }
        other => {
            return Err(SystemError::Platform(format!(
                "NetworkManager reports unknown networking state {other}"
            )));
        }
    }

    // Connection types are NetworkManager setting names (`nm-setting-*.h`).
    Ok(match primary_connection_type {
        "" => {
            return Err(SystemError::Platform(String::from(
                "NetworkManager reports an internet connection but no primary connection",
            )));
        }
        "802-11-wireless" => ConnectionType::Wifi,
        "802-3-ethernet" => ConnectionType::Ethernet,
        "gsm" | "cdma" => ConnectionType::Cellular,
        "bluetooth" => ConnectionType::Bluetooth,
        "vpn" | "wireguard" | "tun" => ConnectionType::Vpn,
        // Bridges, bonds, VLANs, PPPoE, InfiniBand and the rest are not one
        // of the transports this crate names.
        _ => ConnectionType::Other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_wifi_is_connected_wifi() {
        assert_eq!(
            transport(state::CONNECTED_GLOBAL, "802-11-wireless").unwrap(),
            ConnectionType::Wifi
        );
    }

    #[test]
    fn transports_follow_the_primary_connection_type() {
        for (kind, expected) in [
            ("802-3-ethernet", ConnectionType::Ethernet),
            ("gsm", ConnectionType::Cellular),
            ("cdma", ConnectionType::Cellular),
            ("bluetooth", ConnectionType::Bluetooth),
            ("vpn", ConnectionType::Vpn),
            ("wireguard", ConnectionType::Vpn),
            ("tun", ConnectionType::Vpn),
            ("bridge", ConnectionType::Other),
            ("pppoe", ConnectionType::Other),
        ] {
            assert_eq!(
                transport(state::CONNECTED_GLOBAL, kind).unwrap(),
                expected,
                "{kind}"
            );
        }
    }

    #[test]
    fn states_without_internet_are_offline() {
        for state in [
            state::ASLEEP,
            state::DISCONNECTED,
            state::DISCONNECTING,
            state::CONNECTING,
            state::CONNECTED_LOCAL,
            state::CONNECTED_SITE,
        ] {
            // The primary connection of a captive portal network is still the
            // Wi-Fi it sits behind.
            assert_eq!(
                transport(state, "802-11-wireless").unwrap(),
                ConnectionType::None,
                "{state}"
            );
        }
        assert_eq!(
            transport(state::DISCONNECTED, "").unwrap(),
            ConnectionType::None
        );
    }

    #[test]
    fn unknown_states_are_errors() {
        assert!(transport(state::UNKNOWN, "").is_err());
        assert!(transport(55, "802-3-ethernet").is_err());
    }

    #[test]
    fn global_state_without_primary_connection_is_an_error() {
        assert!(transport(state::CONNECTED_GLOBAL, "").is_err());
    }
}
