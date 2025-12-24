import Foundation
import Network

public func get_apple_connectivity() -> RustConnectivityInfo {
    let monitor = NWPathMonitor()
    let queue = DispatchQueue(label: "NetworkCanvas")
    var path: NWPath?
    let semaphore = DispatchSemaphore(value: 0)
    
    monitor.pathUpdateHandler = { p in
        path = p
        semaphore.signal()
    }
    monitor.start(queue: queue)
    _ = semaphore.wait(timeout: .now() + 0.1) // Quick check
    monitor.cancel()

    guard let p = path else {
        return RustConnectivityInfo(connection_type: .None, is_connected: false)
    }

    if p.status != .satisfied {
        return RustConnectivityInfo(connection_type: .None, is_connected: false)
    }

    var type: ConnectionType = .Other
    if p.usesInterfaceType(.wifi) {
        type = .Wifi
    } else if p.usesInterfaceType(.cellular) {
        type = .Cellular
    } else if p.usesInterfaceType(.wiredEthernet) {
        type = .Ethernet
    }

    return RustConnectivityInfo(connection_type: type, is_connected: true)
}

public func get_apple_thermal_state() -> ThermalState {
    let state = ProcessInfo.processInfo.thermalState
    var rustState: ThermalState = .Nominal
    switch state {
    case .nominal: rustState = .Nominal
    case .fair: rustState = .Fair
    case .serious: rustState = .Serious
    case .critical: rustState = .Critical
    @unknown default: rustState = .Unknown
    }
    return rustState
}

public func get_apple_system_load() -> RustSystemLoad {
    let cpuUsage = getHostCPUUsage()
    let memTotal = ProcessInfo.processInfo.physicalMemory
    let memUsed = getUsedMemory()
    
    return RustSystemLoad(cpu_usage: cpuUsage, memory_used: memUsed, memory_total: memTotal)
}

// MARK: - CPU Usage via host_processor_info

private var previousCPUInfo: host_cpu_load_info?
private var previousCPUInfoLock = NSLock()

private func getHostCPUUsage() -> Float {
    var numCPUs: natural_t = 0
    var cpuInfo: processor_info_array_t?
    var numCPUInfo: mach_msg_type_number_t = 0
    
    let result = host_processor_info(
        mach_host_self(),
        PROCESSOR_CPU_LOAD_INFO,
        &numCPUs,
        &cpuInfo,
        &numCPUInfo
    )
    
    guard result == KERN_SUCCESS, let cpuInfo = cpuInfo else {
        return 0.0
    }
    
    defer {
        let size = vm_size_t(numCPUInfo) * vm_size_t(MemoryLayout<integer_t>.size)
        vm_deallocate(mach_task_self_, vm_address_t(bitPattern: cpuInfo), size)
    }
    
    var totalUser: Int32 = 0
    var totalSystem: Int32 = 0
    var totalIdle: Int32 = 0
    var totalNice: Int32 = 0
    
    for i in 0..<Int(numCPUs) {
        let offset = Int32(CPU_STATE_MAX) * Int32(i)
        totalUser += cpuInfo[Int(offset + CPU_STATE_USER)]
        totalSystem += cpuInfo[Int(offset + CPU_STATE_SYSTEM)]
        totalIdle += cpuInfo[Int(offset + CPU_STATE_IDLE)]
        totalNice += cpuInfo[Int(offset + CPU_STATE_NICE)]
    }
    
    let totalTicks = totalUser + totalSystem + totalIdle + totalNice
    let usedTicks = totalUser + totalSystem + totalNice
    
    previousCPUInfoLock.lock()
    defer { previousCPUInfoLock.unlock() }
    
    if let prev = previousCPUInfo {
        let prevTotal = Int32(prev.cpu_ticks.0 + prev.cpu_ticks.1 + prev.cpu_ticks.2 + prev.cpu_ticks.3)
        let prevUsed = Int32(prev.cpu_ticks.0 + prev.cpu_ticks.1 + prev.cpu_ticks.3)
        
        let diffTotal = totalTicks - prevTotal
        let diffUsed = usedTicks - prevUsed
        
        previousCPUInfo = host_cpu_load_info(cpu_ticks: (UInt32(totalUser), UInt32(totalSystem), UInt32(totalIdle), UInt32(totalNice)))
        
        if diffTotal > 0 {
            return Float(diffUsed) / Float(diffTotal) * 100.0
        }
    }
    
    previousCPUInfo = host_cpu_load_info(cpu_ticks: (UInt32(totalUser), UInt32(totalSystem), UInt32(totalIdle), UInt32(totalNice)))
    
    // First call - return instantaneous usage
    if totalTicks > 0 {
        return Float(usedTicks) / Float(totalTicks) * 100.0
    }
    return 0.0
}

// MARK: - Memory via host_statistics64

private func getUsedMemory() -> UInt64 {
    var stats = vm_statistics64()
    var count = mach_msg_type_number_t(MemoryLayout<vm_statistics64>.size / MemoryLayout<integer_t>.size)
    
    let result = withUnsafeMutablePointer(to: &stats) {
        $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
            host_statistics64(mach_host_self(), HOST_VM_INFO64, $0, &count)
        }
    }
    
    guard result == KERN_SUCCESS else {
        return 0
    }
    
    let pageSize = UInt64(vm_kernel_page_size)
    let activeMemory = UInt64(stats.active_count) * pageSize
    let wiredMemory = UInt64(stats.wire_count) * pageSize
    let compressedMemory = UInt64(stats.compressor_page_count) * pageSize
    
    // Used = active + wired + compressed (similar to Activity Monitor)
    return activeMemory + wiredMemory + compressedMemory
}

