//! Windows connectivity.
//!
//! The Network List Manager is Windows' own judgement of whether the machine
//! reaches the internet, the same one the taskbar shows. It does not name the
//! transport, so when it reports internet connectivity the IP Helper adapter
//! table supplies the type of the adapter that carries it.

use crate::sys::network::adapters::{self, Adapter};
use crate::{ConnectionType, SystemError};
use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_NO_DATA, NO_ERROR, WIN32_ERROR};
use windows::Win32::NetworkManagement::IpHelper::{
    GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER,
    GAA_FLAG_SKIP_FRIENDLY_NAME, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses,
    IP_ADAPTER_ADDRESSES_LH,
};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Networking::NetworkListManager::{
    INetworkListManager, NLM_CONNECTIVITY_IPV4_INTERNET, NLM_CONNECTIVITY_IPV6_INTERNET,
    NetworkListManager,
};
use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC};
use windows::Win32::System::Com::{
    CLSCTX_ALL, CO_MTA_USAGE_COOKIE, CoCreateInstance, CoDecrementMTAUsage, CoIncrementMTAUsage,
};

/// The adapter table size Microsoft recommends starting with, which holds a
/// typical machine's table in one call.
const INITIAL_ADAPTER_TABLE_BYTES: u32 = 15 * 1024;

pub fn transport() -> Result<ConnectionType, SystemError> {
    if !connected_to_internet()? {
        return Ok(ConnectionType::None);
    }
    adapters::internet_transport(&adapter_table()?)
}

fn connected_to_internet() -> Result<bool, SystemError> {
    // The calling thread may never have initialised COM; keeping the
    // process's multithreaded apartment alive for the call lets it use COM
    // either way, without changing the apartment of a thread that has one.
    let _apartment = MultithreadedApartment::enter()?;
    // SAFETY: COM is usable on this thread for as long as `_apartment` lives.
    let manager: INetworkListManager =
        unsafe { CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL) }.map_err(|error| {
            SystemError::Platform(format!("creating the Network List Manager: {error}"))
        })?;
    // SAFETY: `manager` is a live Network List Manager.
    let connectivity = unsafe { manager.GetConnectivity() }.map_err(|error| {
        SystemError::Platform(format!(
            "INetworkListManager::GetConnectivity failed: {error}"
        ))
    })?;
    Ok(connectivity.0 & (NLM_CONNECTIVITY_IPV4_INTERNET.0 | NLM_CONNECTIVITY_IPV6_INTERNET.0) != 0)
}

/// A hold on the process's multithreaded apartment.
#[derive(Debug)]
struct MultithreadedApartment(CO_MTA_USAGE_COOKIE);

impl MultithreadedApartment {
    fn enter() -> Result<Self, SystemError> {
        // SAFETY: the cookie is released exactly once, in `drop`.
        unsafe { CoIncrementMTAUsage() }
            .map(Self)
            .map_err(|error| SystemError::Platform(format!("CoIncrementMTAUsage failed: {error}")))
    }
}

impl Drop for MultithreadedApartment {
    fn drop(&mut self) {
        // SAFETY: the cookie came from `CoIncrementMTAUsage` and is released
        // only here.
        if let Err(error) = unsafe { CoDecrementMTAUsage(self.0) } {
            tracing::error!(%error, "CoDecrementMTAUsage failed");
        }
    }
}

/// The adapters `GetAdaptersAddresses` reports, with their default gateways.
fn adapter_table() -> Result<Vec<Adapter>, SystemError> {
    let flags = GAA_FLAG_INCLUDE_GATEWAYS
        | GAA_FLAG_SKIP_ANYCAST
        | GAA_FLAG_SKIP_MULTICAST
        | GAA_FLAG_SKIP_DNS_SERVER
        | GAA_FLAG_SKIP_FRIENDLY_NAME;
    let mut size = INITIAL_ADAPTER_TABLE_BYTES;
    // `u64` elements give the buffer the alignment the adapter records need.
    let mut buffer: Vec<u64>;
    loop {
        buffer = vec![0; (size as usize).div_ceil(size_of::<u64>())];
        // SAFETY: `buffer` holds at least `size` bytes, aligned for
        // `IP_ADAPTER_ADDRESSES_LH`.
        let status = WIN32_ERROR(unsafe {
            GetAdaptersAddresses(
                u32::from(AF_UNSPEC.0),
                flags,
                None,
                Some(buffer.as_mut_ptr().cast()),
                &raw mut size,
            )
        });
        match status {
            NO_ERROR => break,
            // The table outgrew the buffer; `size` now holds the size it
            // needs.
            ERROR_BUFFER_OVERFLOW => {}
            // No adapter has an address.
            ERROR_NO_DATA => return Ok(Vec::new()),
            error => {
                return Err(SystemError::Platform(format!(
                    "GetAdaptersAddresses failed: {}",
                    windows::core::Error::from(error.to_hresult())
                )));
            }
        }
    }

    let mut table = Vec::new();
    let mut cursor = buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
    // SAFETY: `GetAdaptersAddresses` filled `buffer` with a linked list of
    // adapter records whose pointers all point into `buffer`, which outlives
    // this walk.
    while let Some(adapter) = unsafe { cursor.as_ref() } {
        let (mut ipv4_gateway, mut ipv6_gateway) = (false, false);
        let mut gateway = adapter.FirstGatewayAddress;
        // SAFETY: as above, for the adapter's gateway list.
        while let Some(entry) = unsafe { gateway.as_ref() } {
            // SAFETY: every gateway entry carries a socket address.
            let family = unsafe { (*entry.Address.lpSockaddr).sa_family };
            ipv4_gateway |= family == AF_INET;
            ipv6_gateway |= family == AF_INET6;
            gateway = entry.Next;
        }
        table.push(Adapter {
            if_type: adapter.IfType,
            up: adapter.OperStatus == IfOperStatusUp,
            ipv4_gateway_metric: ipv4_gateway.then_some(adapter.Ipv4Metric),
            ipv6_gateway_metric: ipv6_gateway.then_some(adapter.Ipv6Metric),
        });
        cursor = adapter.Next;
    }
    Ok(table)
}
