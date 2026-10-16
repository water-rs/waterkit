//! Linux connectivity.
//!
//! `NetworkManager` is the source wherever it runs: it owns the interfaces it
//! manages and runs its own internet connectivity check, so its answer is the
//! one the desktop shows. A system without it — a `systemd-networkd` server, a
//! container, a sandbox without a system bus — has nothing above the kernel
//! that holds the network state, so there the kernel's interface and route
//! tables are the source. Which source applies is decided by what the system
//! has before any network state is queried. A query that fails is an error,
//! never a cue to ask the other source.

use crate::sys::network::{kernel, network_manager};
use crate::{ConnectionType, SystemError};
use std::io;
use std::path::Path;
use zbus::address::transport::{Transport, UnixSocket};
use zbus::blocking::Connection;
use zbus::blocking::connection::Builder;
use zbus::blocking::fdo::{DBusProxy, PropertiesProxy};
use zbus::names::{InterfaceName, WellKnownName};

/// The D-Bus name `NetworkManager` owns on the system bus.
const NM_SERVICE: &str = "org.freedesktop.NetworkManager";
/// `NetworkManager`'s root object.
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
/// The interface carrying `State` and `PrimaryConnectionType`.
const NM_INTERFACE: &str = "org.freedesktop.NetworkManager";

/// Where this system's network state lives.
enum Source {
    /// `NetworkManager`, reached over this system bus connection.
    NetworkManager(Connection),
    /// The kernel's interface and route tables.
    Kernel,
}

pub fn transport() -> Result<ConnectionType, SystemError> {
    match source()? {
        Source::NetworkManager(bus) => {
            tracing::debug!("reading connectivity from NetworkManager");
            network_manager_transport(&bus)
        }
        Source::Kernel => {
            tracing::debug!("NetworkManager is not running; reading connectivity from the kernel");
            kernel::transport(&SystemFiles)
        }
    }
}

/// `NetworkManager` when it runs on the system bus, the kernel otherwise.
fn source() -> Result<Source, SystemError> {
    let address = zbus::Address::system().map_err(|error| {
        SystemError::Platform(format!("resolving the system bus address: {error}"))
    })?;
    // A system without a bus daemon has no socket at the bus address, and
    // without a bus there is no NetworkManager.
    if let Transport::Unix(unix) = address.transport()
        && let UnixSocket::File(socket) = unix.path()
        && !socket.try_exists().map_err(|error| {
            SystemError::Platform(format!(
                "inspecting the system bus socket {}: {error}",
                socket.display()
            ))
        })?
    {
        return Ok(Source::Kernel);
    }

    let bus = Builder::address(address)
        .and_then(Builder::build)
        .map_err(|error| SystemError::Platform(format!("connecting to the system bus: {error}")))?;
    let asking = |error: &dyn std::fmt::Display| {
        SystemError::Platform(format!(
            "asking the system bus whether NetworkManager runs: {error}"
        ))
    };
    let running = DBusProxy::new(&bus)
        .map_err(|error| asking(&error))?
        .name_has_owner(WellKnownName::from_static_str_unchecked(NM_SERVICE).into())
        .map_err(|error| asking(&error))?;
    Ok(if running {
        Source::NetworkManager(bus)
    } else {
        Source::Kernel
    })
}

fn network_manager_transport(bus: &Connection) -> Result<ConnectionType, SystemError> {
    let failed = |error: &dyn std::fmt::Display| {
        SystemError::Platform(format!("reading NetworkManager's state: {error}"))
    };
    let properties = PropertiesProxy::builder(bus)
        .destination(NM_SERVICE)
        .and_then(|builder| builder.path(NM_PATH))
        .and_then(zbus::blocking::proxy::Builder::build)
        .map_err(|error| failed(&error))?;
    // One call, so the state and the primary connection describe the same
    // moment.
    let mut all = properties
        .get_all(InterfaceName::from_static_str_unchecked(NM_INTERFACE))
        .map_err(|error| failed(&error))?;
    let mut take = |name: &str| {
        all.remove(name)
            .ok_or_else(|| failed(&format!("no {name} property")))
    };
    let state = take("State")?;
    let primary_connection_type = take("PrimaryConnectionType")?;
    network_manager::transport(
        u32::try_from(state).map_err(|error| failed(&format!("State: {error}")))?,
        &String::try_from(primary_connection_type)
            .map_err(|error| failed(&format!("PrimaryConnectionType: {error}")))?,
    )
}

/// The real `/proc` and `/sys`.
#[derive(Debug)]
struct SystemFiles;

impl kernel::KernelFiles for SystemFiles {
    fn read(&self, path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn exists(&self, path: &Path) -> io::Result<bool> {
        path.try_exists()
    }
}
