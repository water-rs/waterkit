//! `Network.framework` path monitoring via in-crate declarations of the seven
//! C functions used (`objc2-network` is unreleased and a git pin would drag a
//! second copy of `objc2`/`block2`/`objc2-encode` into every consumer).

use core::ffi::c_void;
use core::time::Duration;
use std::mem::ManuallyDrop;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use block2::{Block, RcBlock};
use dispatch2::{DispatchQueue, DispatchQueueAttr};
use objc2_foundation::{NSProcessInfo, NSProcessInfoThermalState};

use crate::{ConnectionType, ConnectivityInfo, SystemError, SystemLoad, ThermalState};

/// `nw_path_monitor_t` — opaque `Network.framework` object.
#[repr(C)]
struct NwPathMonitor {
    _private: [u8; 0],
}

/// `nw_path_t` — opaque `Network.framework` object.
#[repr(C)]
struct NwPath {
    _private: [u8; 0],
}

/// `nw_path_status_t` (Network/Headers/NWPathMonitor.h): the path cannot
/// reach the destination.
type NwPathStatusT = i32;
/// `nw_path_status_invalid = 0` — the path is in an invalid state.
#[expect(dead_code, reason = "full constant set documented for completeness")]
const NW_PATH_STATUS_INVALID: NwPathStatusT = 0;
/// `nw_path_status_satisfied = 1` — the path is ready for data transfer.
const NW_PATH_STATUS_SATISFIED: NwPathStatusT = 1;
/// `nw_path_status_unsatisfied = 2` — the interface cannot reach the
/// destination.
#[expect(dead_code, reason = "full constant set documented for completeness")]
const NW_PATH_STATUS_UNSATISFIED: NwPathStatusT = 2;
/// `nw_path_status_satisfiable = 3` — the path could satisfy if a route were
/// established (e.g. waiting on connectivity).
#[expect(dead_code, reason = "full constant set documented for completeness")]
const NW_PATH_STATUS_SATISFIABLE: NwPathStatusT = 3;

/// `nw_interface_type_t` (Network/Headers/NWPath.h): interface kinds a path
/// may use.
type NwInterfaceTypeT = i32;
/// `nw_interface_type_other = 0` — any interface not listed below.
#[expect(dead_code, reason = "full constant set documented for completeness")]
const NW_INTERFACE_TYPE_OTHER: NwInterfaceTypeT = 0;
/// `nw_interface_type_wifi = 1` — IEEE 802.11 wireless.
const NW_INTERFACE_TYPE_WIFI: NwInterfaceTypeT = 1;
/// `nw_interface_type_cellular = 2` — cellular radio.
const NW_INTERFACE_TYPE_CELLULAR: NwInterfaceTypeT = 2;
/// `nw_interface_type_wired = 3` — wired ethernet (also used for USB tethering
/// class links).
const NW_INTERFACE_TYPE_WIRED: NwInterfaceTypeT = 3;
/// `nw_interface_type_loopback = 4` — loopback interface.
#[expect(dead_code, reason = "full constant set documented for completeness")]
const NW_INTERFACE_TYPE_LOOPBACK: NwInterfaceTypeT = 4;

#[link(name = "Network", kind = "framework")]
unsafe extern "C-unwind" {
    fn nw_path_monitor_create() -> *mut NwPathMonitor;
    fn nw_path_monitor_set_queue(monitor: *mut NwPathMonitor, queue: &DispatchQueue);
    fn nw_path_monitor_set_update_handler(
        monitor: *mut NwPathMonitor,
        update_handler: &Block<dyn Fn(*mut c_void)>,
    );
    fn nw_path_monitor_start(monitor: *mut NwPathMonitor);
    fn nw_path_monitor_cancel(monitor: *mut NwPathMonitor);
    fn nw_path_get_status(path: *mut NwPath) -> NwPathStatusT;
    fn nw_path_uses_interface_type(path: *mut NwPath, interface_type: NwInterfaceTypeT) -> bool;
    /// `nw_release` (`nw_object.h`): releases a `Network.framework` object.
    fn nw_release(object: *mut c_void);
}

unsafe extern "C-unwind" {
    /// `dispatch_async_f` (dispatch/queue.h, libSystem): schedules
    /// `work(context)` on `queue` — the function-pointer sibling of
    /// `dispatch_async`, usable where a block is not.
    fn dispatch_async_f(
        queue: &DispatchQueue,
        context: *mut c_void,
        work: extern "C-unwind" fn(*mut c_void),
    );
}

/// Owning `nw_path_monitor_t`: `Drop` cancels the monitor and releases it.
struct PathMonitor(NonNull<NwPathMonitor>);

/// Cancels then releases the monitor. Callable both from `Drop` and as a
/// `dispatch_async_f` work item (the deferred release path used from inside
/// the update handler).
extern "C-unwind" fn release_path_monitor(context: *mut c_void) {
    // SAFETY: `context` is the `nw_path_monitor_t` a `PathMonitor` or its
    // update handler handed over.
    unsafe {
        nw_path_monitor_cancel(context.cast::<NwPathMonitor>());
        nw_release(context);
    }
}

impl Drop for PathMonitor {
    fn drop(&mut self) {
        release_path_monitor(self.0.as_ptr().cast());
    }
}

/// How far apart `load` takes its two CPU tick samples, matching the
/// `sysinfo::MINIMUM_CPU_UPDATE_INTERVAL` the desktop path waits between its
/// own samples.
const CPU_SAMPLE_INTERVAL: Duration = Duration::from_millis(200);

fn read_path(path: *mut NwPath) -> (ConnectionType, bool) {
    // SAFETY: `nw_path_monitor` invokes its update handler with a non-null
    // `nw_path_t` while the monitor runs; `nw_path_get_status` is a pure query.
    let status = unsafe { nw_path_get_status(path) };
    if status != NW_PATH_STATUS_SATISFIED {
        return (ConnectionType::None, false);
    }
    // SAFETY: `nw_path_uses_interface_type` is a pure query on the same path.
    let uses = |interface_type| unsafe { nw_path_uses_interface_type(path, interface_type) };
    let connection_type = if uses(NW_INTERFACE_TYPE_WIFI) {
        ConnectionType::Wifi
    } else if uses(NW_INTERFACE_TYPE_CELLULAR) {
        ConnectionType::Cellular
    } else if uses(NW_INTERFACE_TYPE_WIRED) {
        ConnectionType::Ethernet
    } else {
        ConnectionType::Other
    };
    (connection_type, true)
}

pub async fn connectivity() -> Result<ConnectivityInfo, SystemError> {
    let (tx, rx) = futures::channel::oneshot::channel();
    {
        // Everything dispatch/object-shaped lives in this scope and ends
        // before the `.await`; only the `Send` oneshot receiver crosses it.
        let monitor = PathMonitor(
            // SAFETY: a null return means monitor creation failed.
            NonNull::new(unsafe { nw_path_monitor_create() }).ok_or_else(|| {
                SystemError::Platform("nw_path_monitor_create returned null".into())
            })?,
        );
        let queue = DispatchQueue::new("waterkit.system.connectivity", DispatchQueueAttr::SERIAL);
        let ptr = monitor.0.as_ptr();
        let tx = Arc::new(Mutex::new(Some(tx)));
        let update_handler = {
            let tx = Arc::clone(&tx);
            let queue = queue.clone();
            RcBlock::new(move |path: *mut c_void| {
                let report = read_path(path.cast::<NwPath>());
                let tx = tx.lock().expect("connectivity sender lock").take();
                if let Some(tx) = tx {
                    // The first report is the snapshot. Schedule the deferred
                    // release on the same serial queue: it runs after this
                    // callback, cancels the monitor, and frees it. Sending
                    // happens unconditionally so a dropped receiver still
                    // tears the monitor down.
                    unsafe { dispatch_async_f(&queue, ptr.cast(), release_path_monitor) };
                    let _ = tx.send(report);
                }
            })
        };
        // SAFETY: `queue` serialises handler invocations; `update_handler` is
        // copied by the monitor and invoked only there.
        unsafe { nw_path_monitor_set_update_handler(ptr, &update_handler) };
        unsafe { nw_path_monitor_set_queue(ptr, &queue) };
        unsafe { nw_path_monitor_start(ptr) };
        // The monitor's lifetime is now owned by the update handler's
        // deferred release — `Drop` must not run.
        let _ = ManuallyDrop::new(monitor);
    }
    let (connection_type, is_connected) = rx
        .await
        .expect("the monitor's update handler always answers");
    Ok(ConnectivityInfo::new(connection_type, is_connected))
}

pub fn thermal_state() -> Result<Option<ThermalState>, SystemError> {
    match NSProcessInfo::processInfo().thermalState() {
        NSProcessInfoThermalState::Nominal => Ok(Some(ThermalState::Nominal)),
        NSProcessInfoThermalState::Fair => Ok(Some(ThermalState::Fair)),
        NSProcessInfoThermalState::Serious => Ok(Some(ThermalState::Serious)),
        NSProcessInfoThermalState::Critical => Ok(Some(ThermalState::Critical)),
        other => Err(SystemError::Platform(format!(
            "ProcessInfo reported unknown thermal state {}",
            other.0
        ))),
    }
}

pub fn load() -> Result<SystemLoad, SystemError> {
    let cpu_usage = host_cpu_usage()?;
    let memory_used = used_memory()?;
    let memory_total = NSProcessInfo::processInfo().physicalMemory();
    Ok(SystemLoad::new(Some(cpu_usage), memory_used, memory_total))
}

/// A Mach host call that returned something other than `KERN_SUCCESS`.
fn mach_failure(call: &str, code: libc::kern_return_t) -> SystemError {
    // SAFETY: `mach_error_string` returns a valid C string for any error code.
    let message = unsafe { std::ffi::CStr::from_ptr(libc::mach_error_string(code)) };
    SystemError::Platform(format!(
        "{call} failed: {} ({code})",
        message.to_string_lossy()
    ))
}

/// Reads one `host_statistics`-family record of type `T`.
fn host_statistics_call<T>(
    call: &str,
    flavor: libc::host_flavor_t,
    wide: bool,
) -> Result<T, SystemError> {
    // SAFETY: these stat structs are plain integers and fully overwritten by
    // the call; `MaybeUninit::zeroed` is a valid initialized value for them.
    let mut stats = unsafe { core::mem::MaybeUninit::<T>::zeroed().assume_init() };
    let mut count =
        libc::mach_msg_type_number_t::try_from(size_of::<T>() / size_of::<libc::integer_t>())
            .expect("stat structs are a few integer_t lanes");
    // SAFETY: `stats` is a `T` reinterpreted as `count` `integer_t` lanes,
    // exactly the layout the Mach call fills.
    let code = unsafe {
        let info = (&raw mut stats).cast::<libc::integer_t>();
        let host = mach2::mach_init::mach_host_self();
        if wide {
            libc::host_statistics64(host, flavor, info, &raw mut count)
        } else {
            libc::host_statistics(host, flavor, info, &raw mut count)
        }
    };
    if code == libc::KERN_SUCCESS {
        Ok(stats)
    } else {
        Err(mach_failure(call, code))
    }
}

/// Tick counters (user, system, idle, nice) since boot.
fn host_cpu_ticks() -> Result<[u64; 4], SystemError> {
    let info: libc::host_cpu_load_info = host_statistics_call(
        "host_statistics(HOST_CPU_LOAD_INFO)",
        libc::HOST_CPU_LOAD_INFO,
        false,
    )?;
    Ok(info.cpu_ticks.map(u64::from))
}

/// System-wide CPU usage across the sample window, or since boot if the
/// kernel's counters did not advance.
fn host_cpu_usage() -> Result<f32, SystemError> {
    let first = host_cpu_ticks()?;
    std::thread::sleep(CPU_SAMPLE_INTERVAL);
    let second = host_cpu_ticks()?;

    // The kernel's counters are 32-bit and wrap; a wrapping difference per
    // counter stays correct across one wrap.
    let since = core::array::from_fn(|i| second[i].wrapping_sub(first[i]));
    let elapsed = if since.iter().sum::<u64>() > 0 {
        since
    } else {
        second
    };
    let total = elapsed.iter().sum::<u64>();
    if total == 0 {
        return Err(SystemError::Platform(String::from(
            "host_statistics(HOST_CPU_LOAD_INFO) reported zero CPU ticks",
        )));
    }
    let idle = elapsed[libc::CPU_STATE_IDLE as usize];
    #[expect(
        clippy::cast_precision_loss,
        reason = "CPU ticks fit comfortably in an f32's range at any uptime"
    )]
    Ok((total - idle) as f32 / total as f32 * 100.0)
}

/// Used memory: active + wired + compressed pages (similar to Activity
/// Monitor).
fn used_memory() -> Result<u64, SystemError> {
    let stats: libc::vm_statistics64 = host_statistics_call(
        "host_statistics64(HOST_VM_INFO64)",
        libc::HOST_VM_INFO64,
        true,
    )?;
    // SAFETY: `vm_kernel_page_size` is a read-only export of the kernel's
    // fixed page size.
    let page_size = u64::try_from(unsafe { mach2::vm_page_size::vm_kernel_page_size })
        .expect("kernel page size fits u64");
    Ok((u64::from(stats.active_count)
        + u64::from(stats.wire_count)
        + u64::from(stats.compressor_page_count))
        * page_size)
}
