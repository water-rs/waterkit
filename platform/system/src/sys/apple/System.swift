import Foundation
import Network

/// How long, in seconds, `get_apple_connectivity` waits for `NWPathMonitor` to
/// report the current path. The monitor reports it as soon as it starts, so
/// running out of this is a failure, not a slow network.
private let pathReportTimeoutSeconds = 1

public func get_apple_connectivity() -> ConnectivityResult {
    let monitor = NWPathMonitor()
    let queue = DispatchQueue(label: "waterkit.system.connectivity")
    var path: NWPath?
    let semaphore = DispatchSemaphore(value: 0)

    monitor.pathUpdateHandler = { update in
        // Only the first report is the snapshot; the handler keeps firing on
        // the queue until the monitor is cancelled.
        if path == nil {
            path = update
            semaphore.signal()
        }
    }
    monitor.start(queue: queue)
    let waited = semaphore.wait(timeout: .now() + .seconds(pathReportTimeoutSeconds))
    monitor.cancel()

    guard waited == .success, let p = path else {
        return .Failed("NWPathMonitor reported no network path within \(pathReportTimeoutSeconds) s".intoRustString())
    }

    if p.status != .satisfied {
        return .Reported(RustConnectivityInfo(connection_type: .None, is_connected: false))
    }

    var type: ConnectionType = .Other
    if p.usesInterfaceType(.wifi) {
        type = .Wifi
    } else if p.usesInterfaceType(.cellular) {
        type = .Cellular
    } else if p.usesInterfaceType(.wiredEthernet) {
        type = .Ethernet
    }

    return .Reported(RustConnectivityInfo(connection_type: type, is_connected: true))
}

/// `ProcessInfo.ThermalState.rawValue`; the Rust side maps it and rejects a
/// state it does not know.
public func get_apple_thermal_state() -> Int {
    ProcessInfo.processInfo.thermalState.rawValue
}

public func get_apple_system_load() -> SystemLoadResult {
    do throws(MachCallFailed) {
        let cpuUsage = try hostCPUUsage()
        let memUsed = try usedMemory()
        let memTotal = ProcessInfo.processInfo.physicalMemory
        return .Measured(RustSystemLoad(cpu_usage: cpuUsage, memory_used: memUsed, memory_total: memTotal))
    } catch {
        return .Failed(error.description.intoRustString())
    }
}

/// A Mach host call that returned something other than `KERN_SUCCESS`.
private struct MachCallFailed: Error, CustomStringConvertible {
    let call: String
    let code: kern_return_t

    var description: String {
        "\(call) failed: \(String(cString: mach_error_string(code))) (\(code))"
    }
}

/// Reads one `host_statistics`-family record of type `T` into `stats`.
private func hostStatistics<T>(
    _ call: String,
    _ stats: inout T,
    _ read: (UnsafeMutablePointer<integer_t>, inout mach_msg_type_number_t) -> kern_return_t
) throws(MachCallFailed) {
    var count = mach_msg_type_number_t(MemoryLayout<T>.size / MemoryLayout<integer_t>.size)
    let result = withUnsafeMutablePointer(to: &stats) {
        $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
            read($0, &count)
        }
    }
    guard result == KERN_SUCCESS else {
        throw MachCallFailed(call: call, code: result)
    }
}

// MARK: - CPU Usage via host_statistics

/// Tick counters (user, system, idle, nice) from the previous sample, for
/// the delta.
private var previousCPUTicks: [UInt32]?
private let previousCPUTicksLock = NSLock()

/// System-wide CPU usage since the previous call, or since boot on the first.
private func hostCPUUsage() throws(MachCallFailed) -> Float {
    var info = host_cpu_load_info()
    try hostStatistics("host_statistics(HOST_CPU_LOAD_INFO)", &info) {
        host_statistics(mach_host_self(), HOST_CPU_LOAD_INFO, $0, &$1)
    }
    let ticks = [info.cpu_ticks.0, info.cpu_ticks.1, info.cpu_ticks.2, info.cpu_ticks.3]

    previousCPUTicksLock.lock()
    let previous = previousCPUTicks
    previousCPUTicks = ticks
    previousCPUTicksLock.unlock()

    // The kernel's counters are 32-bit and wrap; a wrapping difference per
    // counter stays correct across one wrap.
    var elapsed = ticks.map(UInt64.init)
    if let previous {
        let since = zip(ticks, previous).map { UInt64($0 &- $1) }
        if since.reduce(0, +) > 0 {
            elapsed = since
        }
    }
    let total = elapsed.reduce(0, +)
    let idle = elapsed[Int(CPU_STATE_IDLE)]
    return Float(total - idle) / Float(total) * 100.0
}

// MARK: - Memory via host_statistics64

private func usedMemory() throws(MachCallFailed) -> UInt64 {
    var stats = vm_statistics64()
    try hostStatistics("host_statistics64(HOST_VM_INFO64)", &stats) {
        host_statistics64(mach_host_self(), HOST_VM_INFO64, $0, &$1)
    }

    let pageSize = UInt64(vm_kernel_page_size)
    let activeMemory = UInt64(stats.active_count) * pageSize
    let wiredMemory = UInt64(stats.wire_count) * pageSize
    let compressedMemory = UInt64(stats.compressor_page_count) * pageSize

    // Used = active + wired + compressed (similar to Activity Monitor)
    return activeMemory + wiredMemory + compressedMemory
}
