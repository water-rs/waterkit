//! Connectivity from the Windows Network List Manager and adapter table.
//!
//! The Network List Manager decides whether the machine reaches the internet;
//! when it does, the transport is the type of the adapter Windows routes
//! through: of the adapters that are operationally up, are not the software
//! loopback, and have a default gateway, the one with the lowest interface
//! metric.

use crate::{ConnectionType, SystemError};

/// IANA `ifType` values, as `IP_ADAPTER_ADDRESSES::IfType` reports them.
mod if_type {
    pub const ETHERNET_CSMACD: u32 = 6;
    pub const SOFTWARE_LOOPBACK: u32 = 24;
    pub const PROP_VIRTUAL: u32 = 53;
    pub const IEEE80211: u32 = 71;
    pub const TUNNEL: u32 = 131;
    pub const WWANPP: u32 = 243;
    pub const WWANPP2: u32 = 244;
}

/// One entry of the `GetAdaptersAddresses` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Adapter {
    /// IANA interface type.
    pub if_type: u32,
    /// `OperStatus` is `IfOperStatusUp`.
    pub up: bool,
    /// The IPv4 interface metric, when the adapter has an IPv4 default
    /// gateway.
    pub ipv4_gateway_metric: Option<u32>,
    /// The IPv6 interface metric, when the adapter has an IPv6 default
    /// gateway.
    pub ipv6_gateway_metric: Option<u32>,
}

impl Adapter {
    /// The metric Windows ranks this adapter's default routes by, when it can
    /// carry traffic off the machine.
    fn route_metric(&self) -> Option<u32> {
        if !self.up || self.if_type == if_type::SOFTWARE_LOOPBACK {
            return None;
        }
        match (self.ipv4_gateway_metric, self.ipv6_gateway_metric) {
            (Some(ipv4), Some(ipv6)) => Some(ipv4.min(ipv6)),
            (metric, None) | (None, metric) => metric,
        }
    }
}

/// The transport of the internet connection, for a machine the Network List
/// Manager reports connected to the internet.
///
/// # Errors
///
/// [`SystemError::Platform`] when no adapter is up with a default gateway,
/// which contradicts the Network List Manager.
pub fn internet_transport(adapters: &[Adapter]) -> Result<ConnectionType, SystemError> {
    let adapter = adapters
        .iter()
        .filter_map(|adapter| Some((adapter.route_metric()?, adapter)))
        .min_by_key(|(metric, _)| *metric)
        .map(|(_, adapter)| adapter)
        .ok_or_else(|| {
            SystemError::Platform(String::from(
                "the Network List Manager reports internet connectivity, \
                 but no adapter is up with a default gateway",
            ))
        })?;

    Ok(match adapter.if_type {
        if_type::IEEE80211 => ConnectionType::Wifi,
        if_type::ETHERNET_CSMACD => ConnectionType::Ethernet,
        if_type::WWANPP | if_type::WWANPP2 => ConnectionType::Cellular,
        // Tunnel adapters, and the virtual adapters VPN clients such as
        // WireGuard and OpenVPN install.
        if_type::TUNNEL | if_type::PROP_VIRTUAL => ConnectionType::Vpn,
        // Everything else, including PPP, which carries both dial-up VPNs and
        // PPPoE broadband and so names no single transport.
        _ => ConnectionType::Other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOOPBACK: Adapter = Adapter {
        if_type: if_type::SOFTWARE_LOOPBACK,
        up: true,
        ipv4_gateway_metric: None,
        ipv6_gateway_metric: None,
    };
    const WIFI: Adapter = Adapter {
        if_type: if_type::IEEE80211,
        up: true,
        ipv4_gateway_metric: Some(35),
        ipv6_gateway_metric: Some(35),
    };
    const ETHERNET: Adapter = Adapter {
        if_type: if_type::ETHERNET_CSMACD,
        up: true,
        ipv4_gateway_metric: Some(25),
        ipv6_gateway_metric: None,
    };
    /// The Hyper-V virtual switch adapter: up, Ethernet-typed, no gateway.
    const HYPER_V: Adapter = Adapter {
        if_type: if_type::ETHERNET_CSMACD,
        up: true,
        ipv4_gateway_metric: None,
        ipv6_gateway_metric: None,
    };

    #[test]
    fn wifi_is_connected_wifi() {
        assert_eq!(
            internet_transport(&[LOOPBACK, HYPER_V, WIFI]).unwrap(),
            ConnectionType::Wifi
        );
    }

    #[test]
    fn ethernet_with_lower_metric_wins() {
        assert_eq!(
            internet_transport(&[LOOPBACK, WIFI, ETHERNET]).unwrap(),
            ConnectionType::Ethernet
        );
    }

    #[test]
    fn down_adapter_is_not_the_connection() {
        let unplugged = Adapter {
            up: false,
            ..ETHERNET
        };
        assert_eq!(
            internet_transport(&[LOOPBACK, unplugged, WIFI]).unwrap(),
            ConnectionType::Wifi
        );
    }

    #[test]
    fn ipv6_only_gateway_counts() {
        let ipv6_only = Adapter {
            ipv4_gateway_metric: None,
            ipv6_gateway_metric: Some(10),
            ..ETHERNET
        };
        assert_eq!(
            internet_transport(&[WIFI, ipv6_only]).unwrap(),
            ConnectionType::Ethernet
        );
    }

    #[test]
    fn vpn_and_cellular_adapters() {
        let wireguard = Adapter {
            if_type: if_type::PROP_VIRTUAL,
            ipv4_gateway_metric: Some(5),
            ipv6_gateway_metric: None,
            ..WIFI
        };
        assert_eq!(
            internet_transport(&[WIFI, wireguard]).unwrap(),
            ConnectionType::Vpn
        );
        let modem = Adapter {
            if_type: if_type::WWANPP2,
            ..WIFI
        };
        assert_eq!(
            internet_transport(&[modem]).unwrap(),
            ConnectionType::Cellular
        );
    }

    #[test]
    fn loopback_or_virtual_only_machine_has_no_internet_adapter() {
        // The loopback and a gateway-less virtual switch cannot carry the
        // internet connection the Network List Manager reported.
        assert!(internet_transport(&[LOOPBACK, HYPER_V]).is_err());
        assert!(internet_transport(&[]).is_err());
    }
}
