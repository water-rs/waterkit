use core::ffi::c_void;
use core::time::Duration;
use std::sync::{Arc, Mutex};

use block2::{Block, RcBlock};
use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchSemaphore, DispatchTime};
use objc2_foundation::{NSProcessInfo, NSProcessInfoThermalState};
use objc2_network::{NWPath, NWPathMonitor, nw_interface_type_t, nw_path_status_t};

use crate::{ConnectionType, ConnectivityInfo, SystemError, SystemLoad, ThermalState};

// `nw_path_monitor_set_update_handler` redeclared with a block argument a
// Rust closure can satisfy. The generated `set_update_handler` types the
// block as `fn(NonNull<NWPath>)`, but `NWPath` is a plain `nw_object`
// without `RefEncode`, so that signature cannot be produced here;
// `nw_path_t` is a pointer-sized argument, ABI-identical to `*mut c_void`.
unsafe extern "C-unwind" {
    fn nw_path_monitor_set_update_handler(
        monitor: &NWPathMonitor,
        update_handler: &Block<'static, fn(*mut c_void)>,
    );
}

// Kernel page size; `libc` only exposes the user-space `vm_page_size`, which
// can differ from the kernel's under translation layers.
unsafe extern "C" {
    static vm_kernel_page_size: usize;
}

// `libc` marks `mach_host_self` deprecated in favour of the `mach2` crate; it
// is a stable libSystem export.
unsafe extern "C" {
    fn mach_host_self() -> libc::mach_port_t;
}

/// How long `connectivity` waits for `NWPathMonitor` to report the current
/// path. The monitor reports it as soon as it starts, so running out of this
/// is a failure, not a slow network.
const PATH_REPORT_TIMEOUT: Duration = Duration::from_secs(1);

/// How far apart `load` takes its two CPU tick samples, matching the
/// `sysinfo::MINIMUM_CPU_UPDATE_INTERVAL` the desktop path waits between its
/// own samples.
const CPU_SAMPLE_INTERVAL: Duration = Duration::from_millis(200);

fn read_path(path: &NWPath) -> (ConnectionType, bool) {
    if path.status() != nw_path_status_t::satisfied {
        return (ConnectionType::None, false);
    }
    let connection_type = if path.uses_interface_type(nw_interface_type_t::wifi) {
        ConnectionType::Wifi
    } else if path.uses_interface_type(nw_interface_type_t::cellular) {
        ConnectionType::Cellular
    } else if path.uses_interface_type(nw_interface_type_t::wired) {
        ConnectionType::Ethernet
    } else {
        ConnectionType::Other
    };
    (connection_type, true)
}

pub fn connectivity() -> Result<ConnectivityInfo, SystemError> {
    let monitor = NWPathMonitor::new();
    let queue = DispatchQueue::new(
        Some(c"waterkit.system.connectivity"),
        DispatchQueueAttr::SERIAL,
    );
    let semaphore = DispatchSemaphore::new(0);
    let snapshot = Arc::new(Mutex::new(None::<(ConnectionType, bool)>));

    let update_handler = {
        let snapshot = Arc::clone(&snapshot);
        let semaphore = semaphore.clone();
        RcBlock::new(move |path: *mut c_void| {
            // SAFETY: `NWPathMonitor` invokes its update handler with a
            // non-null `nw_path_t` while the monitor is running.
            let path = unsafe { &*path.cast::<NWPath>() };
            let report = read_path(path);
            // Only the first report is the snapshot; the handler keeps firing
            // on the queue until the monitor is cancelled.
            let fresh = {
                let mut slot = snapshot.lock().expect("connectivity snapshot lock");
                let fresh = slot.is_none();
                if fresh {
                    *slot = Some(report);
                }
                fresh
            };
            if fresh {
                let _ = semaphore.signal();
            }
        })
    };
    // SAFETY: `update_handler` outlives the monitor, which is cancelled below
    // before this function returns.
    unsafe { nw_path_monitor_set_update_handler(&monitor, &update_handler) };
    // SAFETY: `queue` is a private serial queue owned by this call.
    unsafe { monitor.set_queue(&queue) };
    monitor.start();

    let waited =
        semaphore.wait(DispatchTime::NOW.time(
            i64::try_from(PATH_REPORT_TIMEOUT.as_nanos()).expect("one second in ns fits i64"),
        ));
    monitor.cancel();

    let reported = (waited == 0)
        .then(|| snapshot.lock().expect("connectivity snapshot lock").take())
        .flatten();
    let Some((connection_type, is_connected)) = reported else {
        return Err(SystemError::Platform(format!(
            "NWPathMonitor reported no network path within {} s",
            PATH_REPORT_TIMEOUT.as_secs()
        )));
    };
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
        let host = mach_host_self();
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
    let page_size = unsafe { vm_kernel_page_size } as u64;
    Ok((u64::from(stats.active_count)
        + u64::from(stats.wire_count)
        + u64::from(stats.compressor_page_count))
        * page_size)
}
