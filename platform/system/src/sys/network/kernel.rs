//! Connectivity from the Linux kernel's own interface and route tables.
//!
//! This is the source on systems where `NetworkManager` does not run. The
//! connection is the interface behind the lowest-metric default route — the
//! one the kernel sends internet traffic through — provided that interface is
//! up and passing traffic. Loopback is recognised by `IFF_LOOPBACK`, never by
//! name, and the transport comes from the interface's device type.

use crate::{ConnectionType, SystemError};
use std::io;
use std::path::Path;

/// The IPv4 routing table.
const IPV4_ROUTES: &str = "/proc/net/route";
/// The IPv6 routing table; present only while the kernel has IPv6 enabled.
const IPV6_ROUTES: &str = "/proc/net/ipv6_route";
/// One directory per network interface.
const NET_CLASS: &str = "/sys/class/net";

/// `IFF_UP` from `<linux/if.h>`: the interface is administratively up.
const IFF_UP: u32 = 0x1;
/// `IFF_LOOPBACK` from `<linux/if.h>`.
const IFF_LOOPBACK: u32 = 0x8;
/// `RTF_UP` from `<linux/route.h>`: the route is usable.
const RTF_UP: u32 = 0x0001;
/// `RTF_REJECT` from `<linux/route.h>`: the route rejects traffic, such as the
/// unreachable IPv6 default route the kernel installs on `lo`.
const RTF_REJECT: u32 = 0x0200;
/// `ARPHRD_ETHER` from `<linux/if_arp.h>`.
const ARPHRD_ETHER: u16 = 1;

/// Read access to `/proc` and `/sys`.
pub trait KernelFiles {
    /// The contents of the file at `path`.
    ///
    /// # Errors
    ///
    /// The I/O error that reading the file failed with.
    fn read(&self, path: &Path) -> io::Result<String>;

    /// Whether anything exists at `path`.
    ///
    /// # Errors
    ///
    /// The I/O error that inspecting the path failed with.
    fn exists(&self, path: &Path) -> io::Result<bool>;
}

/// The transport of the connection the kernel routes internet traffic
/// through, or [`ConnectionType::None`] when no usable interface carries a
/// default route.
///
/// # Errors
///
/// [`SystemError::Platform`] when a table cannot be read or has a shape this
/// parser does not know.
pub fn transport(files: &impl KernelFiles) -> Result<ConnectionType, SystemError> {
    let mut routes = parse_ipv4_default_routes(&read(files, Path::new(IPV4_ROUTES))?)?;
    // A kernel booted with IPv6 disabled has no IPv6 routing table at all, so
    // there are no IPv6 routes to consider.
    let ipv6_routes = Path::new(IPV6_ROUTES);
    if exists(files, ipv6_routes)? {
        routes.extend(parse_ipv6_default_routes(&read(files, ipv6_routes)?)?);
    }
    // The kernel prefers the lowest metric; the sort is stable, so equal
    // metrics keep the tables' order.
    routes.sort_by_key(|route| route.metric);

    for route in &routes {
        let link = Link::read(files, &route.interface)?;
        if link.passes_traffic() {
            return Ok(link.transport);
        }
    }
    Ok(ConnectionType::None)
}

/// A usable default route.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DefaultRoute {
    interface: String,
    metric: u32,
}

/// Default routes from `/proc/net/route`, located by the table's own header.
fn parse_ipv4_default_routes(table: &str) -> Result<Vec<DefaultRoute>, SystemError> {
    let mut rows = table.lines();
    let header: Vec<&str> = rows
        .next()
        .ok_or_else(|| malformed(IPV4_ROUTES, "the table has no header"))?
        .split_whitespace()
        .collect();
    let column = |name: &str| {
        header
            .iter()
            .position(|column| *column == name)
            .ok_or_else(|| malformed(IPV4_ROUTES, &format!("the header has no {name} column")))
    };
    let (interface, destination, flags, metric, mask) = (
        column("Iface")?,
        column("Destination")?,
        column("Flags")?,
        column("Metric")?,
        column("Mask")?,
    );

    let mut routes = Vec::new();
    for row in rows.filter(|row| !row.trim().is_empty()) {
        let fields: Vec<&str> = row.split_whitespace().collect();
        if fields.len() != header.len() {
            return Err(malformed(IPV4_ROUTES, &format!("row {row:?}")));
        }
        let is_default =
            hex(IPV4_ROUTES, fields[destination])? == 0 && hex(IPV4_ROUTES, fields[mask])? == 0;
        if is_default && is_usable_route(hex(IPV4_ROUTES, fields[flags])?) {
            routes.push(DefaultRoute {
                interface: fields[interface].to_owned(),
                metric: fields[metric]
                    .parse()
                    .map_err(|_| malformed(IPV4_ROUTES, &format!("metric in row {row:?}")))?,
            });
        }
    }
    Ok(routes)
}

/// Default routes from `/proc/net/ipv6_route`, whose rows are, in order: the
/// destination and its prefix length, the source and its prefix length, the
/// next hop, the metric, the reference count, the use count, the flags and the
/// interface.
fn parse_ipv6_default_routes(table: &str) -> Result<Vec<DefaultRoute>, SystemError> {
    let mut routes = Vec::new();
    for row in table.lines().filter(|row| !row.trim().is_empty()) {
        let fields: Vec<&str> = row.split_whitespace().collect();
        let &[destination, prefix, _, _, _, metric, _, _, flags, interface] = fields.as_slice()
        else {
            return Err(malformed(IPV6_ROUTES, &format!("row {row:?}")));
        };
        let is_default = hex(IPV6_ROUTES, prefix)? == 0
            && destination.len() == 32
            && destination.bytes().all(|digit| digit == b'0');
        if is_default && is_usable_route(hex(IPV6_ROUTES, flags)?) {
            routes.push(DefaultRoute {
                interface: interface.to_owned(),
                metric: hex(IPV6_ROUTES, metric)?,
            });
        }
    }
    Ok(routes)
}

const fn is_usable_route(flags: u32) -> bool {
    flags & RTF_UP != 0 && flags & RTF_REJECT == 0
}

/// An interface as `/sys/class/net/<name>` describes it.
#[derive(Debug, Clone, Copy)]
struct Link {
    flags: u32,
    operstate: OperState,
    transport: ConnectionType,
}

impl Link {
    fn read(files: &impl KernelFiles, name: &str) -> Result<Self, SystemError> {
        let directory = Path::new(NET_CLASS).join(name);
        let attribute = |attribute: &str| read(files, &directory.join(attribute));

        let flags_path = directory.join("flags");
        let flags = attribute("flags")?;
        let flags = u32::from_str_radix(flags.trim().trim_start_matches("0x"), 16)
            .map_err(|_| malformed(&flags_path.display().to_string(), &flags))?;
        let operstate = OperState::parse(&directory, &attribute("operstate")?)?;
        let type_path = directory.join("type");
        let arphrd = attribute("type")?;
        let arphrd = arphrd
            .trim()
            .parse()
            .map_err(|_| malformed(&type_path.display().to_string(), &arphrd))?;
        let uevent = attribute("uevent")?;
        let devtype = uevent
            .lines()
            .find_map(|line| line.strip_prefix("DEVTYPE="));
        // `tun_flags` exists exactly on TUN and TAP devices.
        let tun = exists(files, &directory.join("tun_flags"))?;
        // `device` links an interface to the hardware behind it; virtual
        // interfaces such as `veth` pairs have none.
        let hardware = exists(files, &directory.join("device"))?;

        Ok(Self {
            flags,
            operstate,
            transport: link_transport(devtype, arphrd, tun, hardware),
        })
    }

    /// Whether the interface can carry traffic: not loopback, administratively
    /// up, and operationally up.
    const fn passes_traffic(self) -> bool {
        self.flags & IFF_LOOPBACK == 0 && self.flags & IFF_UP != 0 && self.operstate.is_up()
    }
}

/// The transport an interface provides, from the device type the kernel
/// reports for it.
fn link_transport(devtype: Option<&str>, arphrd: u16, tun: bool, hardware: bool) -> ConnectionType {
    match devtype {
        Some("wlan") => ConnectionType::Wifi,
        Some("wwan") => ConnectionType::Cellular,
        // Bluetooth PAN (`bnep`) interfaces.
        Some("bluetooth") => ConnectionType::Bluetooth,
        Some("wireguard") => ConnectionType::Vpn,
        _ if tun => ConnectionType::Vpn,
        // An Ethernet-framed interface with no more specific device type and a
        // device behind it is an Ethernet adapter; bridges, bonds, VLANs and
        // virtual pairs are not a transport of their own.
        None if arphrd == ARPHRD_ETHER && hardware => ConnectionType::Ethernet,
        _ => ConnectionType::Other,
    }
}

/// RFC 2863 operational state, as `/sys/class/net/<name>/operstate` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperState {
    Unknown,
    NotPresent,
    Down,
    LowerLayerDown,
    Testing,
    Dormant,
    Up,
}

impl OperState {
    fn parse(directory: &Path, text: &str) -> Result<Self, SystemError> {
        Ok(match text.trim() {
            "unknown" => Self::Unknown,
            "notpresent" => Self::NotPresent,
            "down" => Self::Down,
            "lowerlayerdown" => Self::LowerLayerDown,
            "testing" => Self::Testing,
            "dormant" => Self::Dormant,
            "up" => Self::Up,
            other => {
                return Err(malformed(
                    &directory.join("operstate").display().to_string(),
                    other,
                ));
            }
        })
    }

    /// The kernel's own `netif_oper_up`: drivers that do not track the link
    /// state, such as TUN devices, report `unknown` and pass traffic.
    const fn is_up(self) -> bool {
        matches!(self, Self::Up | Self::Unknown)
    }
}

fn read(files: &impl KernelFiles, path: &Path) -> Result<String, SystemError> {
    files
        .read(path)
        .map_err(|error| SystemError::Platform(format!("reading {}: {error}", path.display())))
}

fn exists(files: &impl KernelFiles, path: &Path) -> Result<bool, SystemError> {
    files
        .exists(path)
        .map_err(|error| SystemError::Platform(format!("inspecting {}: {error}", path.display())))
}

fn hex(source: &str, field: &str) -> Result<u32, SystemError> {
    u32::from_str_radix(field, 16).map_err(|_| malformed(source, field))
}

fn malformed(source: &str, detail: &str) -> SystemError {
    SystemError::Platform(format!("{source} has an unexpected shape: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    const IPV4_HEADER: &str = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT       \n";
    /// The unreachable default route the kernel installs on `lo`.
    const IPV6_LOOPBACK_REJECT: &str = "00000000000000000000000000000000 00 00000000000000000000000000000000 00 00000000000000000000000000000000 ffffffff 00000001 00000000 00200200       lo\n";

    fn ipv4_default(interface: &str, metric: u32) -> String {
        format!("{interface}\t00000000\t0101A8C0\t0003\t0\t0\t{metric}\t00000000\t0\t0\t0       \n")
    }

    fn ipv4_subnet(interface: &str, metric: u32) -> String {
        format!("{interface}\t0001A8C0\t00000000\t0001\t0\t0\t{metric}\t00FFFFFF\t0\t0\t0       \n")
    }

    fn ipv6_default(interface: &str, metric: u32) -> String {
        format!(
            "00000000000000000000000000000000 00 00000000000000000000000000000000 00 fe800000000000000000000000000001 {metric:08x} 00000001 00000000 00000003 {interface:>8}\n"
        )
    }

    /// An interface's attributes, as the kernel exposes them.
    struct Interface {
        flags: &'static str,
        operstate: &'static str,
        arphrd: u16,
        devtype: Option<&'static str>,
        tun: bool,
        hardware: bool,
    }

    const LOOPBACK: Interface = Interface {
        flags: "0x9",
        operstate: "unknown",
        arphrd: 772,
        devtype: None,
        tun: false,
        hardware: false,
    };
    const WIFI: Interface = Interface {
        flags: "0x1003",
        operstate: "up",
        arphrd: ARPHRD_ETHER,
        devtype: Some("wlan"),
        tun: false,
        hardware: true,
    };
    const ETHERNET: Interface = Interface {
        flags: "0x1003",
        operstate: "up",
        arphrd: ARPHRD_ETHER,
        devtype: None,
        tun: false,
        hardware: true,
    };

    /// A machine's `/proc` and `/sys`, in memory.
    #[derive(Default)]
    struct Machine {
        files: HashMap<PathBuf, String>,
    }

    impl Machine {
        fn new(ipv4_routes: &[String], ipv6_routes: Option<&[String]>) -> Self {
            let mut machine = Self::default();
            machine.files.insert(
                PathBuf::from(IPV4_ROUTES),
                std::iter::once(IPV4_HEADER.to_owned())
                    .chain(ipv4_routes.iter().cloned())
                    .collect(),
            );
            if let Some(ipv6_routes) = ipv6_routes {
                machine
                    .files
                    .insert(PathBuf::from(IPV6_ROUTES), ipv6_routes.concat());
            }
            machine.interface("lo", &LOOPBACK)
        }

        fn interface(mut self, name: &str, interface: &Interface) -> Self {
            let directory = Path::new(NET_CLASS).join(name);
            let devtype = interface
                .devtype
                .map(|devtype| format!("DEVTYPE={devtype}\n"))
                .unwrap_or_default();
            let uevent = format!("INTERFACE={name}\nIFINDEX=2\n{devtype}");
            for (attribute, contents) in [
                ("flags", format!("{}\n", interface.flags)),
                ("operstate", format!("{}\n", interface.operstate)),
                ("type", format!("{}\n", interface.arphrd)),
                ("uevent", uevent),
            ] {
                self.files.insert(directory.join(attribute), contents);
            }
            if interface.tun {
                self.files
                    .insert(directory.join("tun_flags"), String::from("0x1001\n"));
            }
            if interface.hardware {
                self.files.insert(directory.join("device"), String::new());
            }
            self
        }

        fn transport(&self) -> Result<ConnectionType, SystemError> {
            transport(self)
        }
    }

    impl KernelFiles for Machine {
        fn read(&self, path: &Path) -> io::Result<String> {
            self.files
                .get(path)
                .cloned()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }

        fn exists(&self, path: &Path) -> io::Result<bool> {
            Ok(self.files.contains_key(path))
        }
    }

    #[test]
    fn wifi_on_wlo1_is_connected_wifi() {
        let machine = Machine::new(
            &[ipv4_default("wlo1", 600), ipv4_subnet("wlo1", 600)],
            Some(&[ipv6_default("wlo1", 600), IPV6_LOOPBACK_REJECT.to_owned()]),
        )
        .interface("wlo1", &WIFI);
        assert_eq!(machine.transport().unwrap(), ConnectionType::Wifi);
    }

    #[test]
    fn ethernet_is_connected_ethernet() {
        let machine =
            Machine::new(&[ipv4_default("enp3s0", 100)], Some(&[])).interface("enp3s0", &ETHERNET);
        assert_eq!(machine.transport().unwrap(), ConnectionType::Ethernet);
    }

    #[test]
    fn lowest_metric_default_route_wins() {
        let machine = Machine::new(
            &[ipv4_default("wlo1", 600), ipv4_default("enp3s0", 100)],
            Some(&[]),
        )
        .interface("wlo1", &WIFI)
        .interface("enp3s0", &ETHERNET);
        assert_eq!(machine.transport().unwrap(), ConnectionType::Ethernet);
    }

    #[test]
    fn down_adapter_is_not_the_connection() {
        // The wired adapter keeps a stale default route while its cable is
        // unplugged; the kernel reports its lower layer down.
        let unplugged = Interface {
            operstate: "lowerlayerdown",
            ..ETHERNET
        };
        let machine = Machine::new(
            &[ipv4_default("enp3s0", 100), ipv4_default("wlo1", 600)],
            Some(&[]),
        )
        .interface("enp3s0", &unplugged)
        .interface("wlo1", &WIFI);
        assert_eq!(machine.transport().unwrap(), ConnectionType::Wifi);
    }

    #[test]
    fn administratively_down_adapter_is_offline() {
        let disabled = Interface {
            flags: "0x1002",
            operstate: "down",
            ..ETHERNET
        };
        let machine =
            Machine::new(&[ipv4_default("enp3s0", 100)], Some(&[])).interface("enp3s0", &disabled);
        assert_eq!(machine.transport().unwrap(), ConnectionType::None);
    }

    #[test]
    fn associating_wifi_is_offline() {
        let associating = Interface {
            operstate: "dormant",
            ..WIFI
        };
        let machine =
            Machine::new(&[ipv4_default("wlo1", 600)], Some(&[])).interface("wlo1", &associating);
        assert_eq!(machine.transport().unwrap(), ConnectionType::None);
    }

    #[test]
    fn interface_without_default_route_is_offline() {
        // A link-local or LAN-only address is not a connection.
        let machine =
            Machine::new(&[ipv4_subnet("enp3s0", 100)], Some(&[])).interface("enp3s0", &ETHERNET);
        assert_eq!(machine.transport().unwrap(), ConnectionType::None);
    }

    #[test]
    fn loopback_only_machine_is_offline() {
        let machine = Machine::new(&[], Some(&[IPV6_LOOPBACK_REJECT.to_owned()]));
        assert_eq!(machine.transport().unwrap(), ConnectionType::None);
    }

    #[test]
    fn loopback_is_recognised_by_flag_not_name() {
        // A default route on a loopback interface carries no traffic off the
        // machine, whatever the interface is called.
        let machine = Machine::new(&[ipv4_default("dummyroute", 0)], Some(&[]))
            .interface("dummyroute", &LOOPBACK);
        assert_eq!(machine.transport().unwrap(), ConnectionType::None);
    }

    #[test]
    fn virtual_machine_nic_is_ethernet() {
        // A virtio NIC sits on the virtio bus, so it has a device like any
        // physical adapter.
        let machine =
            Machine::new(&[ipv4_default("eth0", 0)], Some(&[])).interface("eth0", &ETHERNET);
        assert_eq!(machine.transport().unwrap(), ConnectionType::Ethernet);
    }

    #[test]
    fn container_veth_is_other() {
        let veth = Interface {
            hardware: false,
            ..ETHERNET
        };
        let machine = Machine::new(&[ipv4_default("eth0", 0)], Some(&[])).interface("eth0", &veth);
        assert_eq!(machine.transport().unwrap(), ConnectionType::Other);
    }

    #[test]
    fn bridge_is_other() {
        let bridge = Interface {
            devtype: Some("bridge"),
            hardware: false,
            ..ETHERNET
        };
        let machine =
            Machine::new(&[ipv4_default("br0", 425)], Some(&[])).interface("br0", &bridge);
        assert_eq!(machine.transport().unwrap(), ConnectionType::Other);
    }

    #[test]
    fn wireguard_and_tun_are_vpn() {
        let wireguard = Interface {
            flags: "0x91",
            operstate: "unknown",
            arphrd: 65534,
            devtype: Some("wireguard"),
            tun: false,
            hardware: false,
        };
        let machine = Machine::new(
            &[ipv4_default("wlo1", 600), ipv4_default("wg0", 50)],
            Some(&[]),
        )
        .interface("wlo1", &WIFI)
        .interface("wg0", &wireguard);
        assert_eq!(machine.transport().unwrap(), ConnectionType::Vpn);

        let tun = Interface {
            devtype: None,
            tun: true,
            ..wireguard
        };
        let machine = Machine::new(&[ipv4_default("tun0", 50)], Some(&[])).interface("tun0", &tun);
        assert_eq!(machine.transport().unwrap(), ConnectionType::Vpn);
    }

    #[test]
    fn wwan_is_cellular_and_bnep_is_bluetooth() {
        let modem = Interface {
            arphrd: 519,
            devtype: Some("wwan"),
            ..ETHERNET
        };
        let machine =
            Machine::new(&[ipv4_default("wwan0", 700)], Some(&[])).interface("wwan0", &modem);
        assert_eq!(machine.transport().unwrap(), ConnectionType::Cellular);

        let pan = Interface {
            devtype: Some("bluetooth"),
            hardware: false,
            ..ETHERNET
        };
        let machine =
            Machine::new(&[ipv4_default("bnep0", 750)], Some(&[])).interface("bnep0", &pan);
        assert_eq!(machine.transport().unwrap(), ConnectionType::Bluetooth);
    }

    #[test]
    fn ipv6_only_connection() {
        let machine =
            Machine::new(&[], Some(&[ipv6_default("wlo1", 1024)])).interface("wlo1", &WIFI);
        assert_eq!(machine.transport().unwrap(), ConnectionType::Wifi);
    }

    #[test]
    fn kernel_without_ipv6_reads_ipv4_only() {
        let machine = Machine::new(&[ipv4_default("wlo1", 600)], None).interface("wlo1", &WIFI);
        assert_eq!(machine.transport().unwrap(), ConnectionType::Wifi);
    }

    #[test]
    fn unreadable_table_is_an_error() {
        let mut machine = Machine::new(&[], None);
        machine.files.remove(Path::new(IPV4_ROUTES));
        let SystemError::Platform(message) = machine.transport().unwrap_err();
        assert!(message.contains(IPV4_ROUTES), "{message}");
    }

    #[test]
    fn malformed_tables_are_errors() {
        let mut machine = Machine::new(&[], None);
        machine.files.insert(
            PathBuf::from(IPV4_ROUTES),
            String::from("Iface\tGateway\nwlo1\t0101A8C0\n"),
        );
        assert!(machine.transport().is_err());

        let machine = Machine::new(&[], Some(&[String::from("0000 00 lo\n")]));
        assert!(machine.transport().is_err());
    }

    #[test]
    fn unknown_operstate_is_an_error() {
        let strange = Interface {
            operstate: "sideways",
            ..ETHERNET
        };
        let machine =
            Machine::new(&[ipv4_default("enp3s0", 100)], Some(&[])).interface("enp3s0", &strange);
        assert!(machine.transport().is_err());
    }
}
