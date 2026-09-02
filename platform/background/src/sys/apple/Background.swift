import BackgroundTasks
import Foundation
import UIKit

private let taskKindAppRefresh: UInt8 = 1
private let taskKindProcessing: UInt8 = 2
private let taskKindContinuedProcessing: UInt8 = 3

private let capabilityAppRefresh: UInt8 = 1 << 0
private let capabilityProcessing: UInt8 = 1 << 1
private let capabilityContinuedProcessing: UInt8 = 1 << 2
private let capabilityLaunchEvents: UInt8 = 1 << 3
private let capabilityContinuedGPU: UInt8 = 1 << 4

private struct RegistrationEntry: Codable {
    let identifier: String
    let kind: UInt8
}

private enum BackgroundBridgeError: Error {
    case lateInitialization(String)
    case configurationMissing(String)
    case schedulerRejected(Int, String)
    case invalidToken(String)
    case notSupported
    case platform(String)

    func encoded() -> RustString {
        switch self {
        case .lateInitialization(let message):
            return RustString("late_init:\(message)")
        case .configurationMissing(let message):
            return RustString("config_missing:\(message)")
        case .schedulerRejected(let code, let message):
            return RustString("scheduler_rejected:\(code):\(message)")
        case .invalidToken(let message):
            return RustString("invalid_token:\(message)")
        case .notSupported:
            return RustString("not_supported")
        case .platform(let message):
            return RustString(message)
        }
    }
}

@available(iOS 13.0, *)
private final class BackgroundRuntimeBridge {
    private let eventCtx: UInt64
    private var runtimeHandle: UInt64 = 0
    private var pendingTasks: [UInt64: BGTask] = [:]
    private var nextTaskToken: UInt64 = 1

    init(eventCtx: UInt64) {
        self.eventCtx = eventCtx
    }

    func bindRuntimeHandle(_ runtimeHandle: UInt64) {
        self.runtimeHandle = runtimeHandle
    }

    func register(_ entries: [RegistrationEntry]) throws {
        for entry in entries {
            let identifier = entry.identifier
            let kind = entry.kind

            let registered = BGTaskScheduler.shared.register(forTaskWithIdentifier: identifier, using: nil) {
                [weak self] task in
                guard let self else {
                    return
                }

                let taskToken = self.allocateTaskToken(task)
                task.expirationHandler = { [weak self] in
                    guard let self else {
                        return
                    }

                    self.removeTaskToken(taskToken)
                    on_background_task_expired_raw(
                        self.eventCtx,
                        self.runtimeHandle,
                        taskToken,
                        identifier,
                        kind
                    )
                }

                on_background_task_launched_raw(
                    self.eventCtx,
                    self.runtimeHandle,
                    taskToken,
                    identifier,
                    kind
                )
            }

            if !registered {
                throw BackgroundBridgeError.configurationMissing(
                    "failed to register task identifier `\(identifier)`; ensure it exists in BGTaskSchedulerPermittedIdentifiers"
                )
            }
        }
    }

    func submitAppRefresh(identifier: String, earliestBeginSeconds: UInt64) throws {
        let request = BGAppRefreshTaskRequest(identifier: identifier)
        if earliestBeginSeconds > 0 {
            request.earliestBeginDate = Date(timeIntervalSinceNow: TimeInterval(earliestBeginSeconds))
        }
        try submitTaskRequest(request)
    }

    func submitProcessing(
        identifier: String,
        earliestBeginSeconds: UInt64,
        requiresNetworkConnectivity: Bool,
        requiresExternalPower: Bool
    ) throws {
        let request = BGProcessingTaskRequest(identifier: identifier)
        if earliestBeginSeconds > 0 {
            request.earliestBeginDate = Date(timeIntervalSinceNow: TimeInterval(earliestBeginSeconds))
        }
        request.requiresNetworkConnectivity = requiresNetworkConnectivity
        request.requiresExternalPower = requiresExternalPower
        try submitTaskRequest(request)
    }

    func submitContinuedProcessing(
        identifier: String,
        title: String,
        subtitle: String,
        strategy: UInt8,
        requiresGPU: Bool
    ) throws {
#if WATERKIT_HAS_IOS26_BACKGROUND_TASKS
        if #available(iOS 26.0, *) {
            let request = BGContinuedProcessingTaskRequest(
                identifier: identifier,
                title: title,
                subtitle: subtitle
            )

            switch strategy {
            case 0:
                request.strategy = .fail
            case 1:
                request.strategy = .queue
            default:
                throw BackgroundBridgeError.configurationMissing(
                    "invalid continued-processing strategy value: \(strategy)"
                )
            }

            if requiresGPU {
                if !BGTaskScheduler.supportedResources.contains(.gpu) {
                    throw BackgroundBridgeError.schedulerRejected(
                        0,
                        "device does not support background GPU continued-processing resources"
                    )
                }
                request.requiredResources = .gpu
            }

            try submitTaskRequest(request)
            return
        }
#endif

        throw BackgroundBridgeError.notSupported
    }

    func cancel(identifier: String) {
        BGTaskScheduler.shared.cancel(taskRequestWithIdentifier: identifier)
    }

    func cancelAll() {
        BGTaskScheduler.shared.cancelAllTaskRequests()
    }

    func completeTask(taskToken: UInt64, success: Bool) throws {
        guard let task = pendingTasks.removeValue(forKey: taskToken) else {
            throw BackgroundBridgeError.invalidToken(
                "unknown task token \(taskToken); it may already be completed or expired"
            )
        }
        task.setTaskCompleted(success: success)
    }

    func updateContinuedStatus(taskToken: UInt64, title: String, subtitle: String) throws {
#if WATERKIT_HAS_IOS26_BACKGROUND_TASKS
        if #available(iOS 26.0, *) {
            guard let continuedTask = pendingTasks[taskToken] as? BGContinuedProcessingTask else {
                throw BackgroundBridgeError.invalidToken(
                    "task token \(taskToken) does not reference a continued processing task"
                )
            }
            continuedTask.updateTitle(title, subtitle: subtitle)
            return
        }
#endif

        throw BackgroundBridgeError.notSupported
    }

    func updateContinuedProgress(taskToken: UInt64, completed: UInt64, total: UInt64) throws {
#if WATERKIT_HAS_IOS26_BACKGROUND_TASKS
        if #available(iOS 26.0, *) {
            guard let continuedTask = pendingTasks[taskToken] as? BGContinuedProcessingTask else {
                throw BackgroundBridgeError.invalidToken(
                    "task token \(taskToken) does not reference a continued processing task"
                )
            }
            continuedTask.progress.totalUnitCount = Int64(total)
            continuedTask.progress.completedUnitCount = Int64(completed)
            return
        }
#endif

        throw BackgroundBridgeError.notSupported
    }

    func shutdown() {
        for (_, task) in pendingTasks {
            task.setTaskCompleted(success: false)
        }
        pendingTasks.removeAll()
    }

    private func allocateTaskToken(_ task: BGTask) -> UInt64 {
        let taskToken = nextTaskToken
        nextTaskToken = nextTaskToken &+ 1
        pendingTasks[taskToken] = task
        return taskToken
    }

    private func removeTaskToken(_ taskToken: UInt64) {
        _ = pendingTasks.removeValue(forKey: taskToken)
    }

    private func submitTaskRequest(_ request: BGTaskRequest) throws {
        do {
            try BGTaskScheduler.shared.submit(request)
        } catch {
            throw mapSchedulerError(error)
        }
    }
}

@available(iOS 13.0, *)
private func runtimeFromHandle(_ runtimeHandle: UInt64) throws -> BackgroundRuntimeBridge {
    guard runtimeHandle != 0,
          let rawPointer = UnsafeRawPointer(bitPattern: UInt(runtimeHandle))
    else {
        throw BackgroundBridgeError.invalidToken("runtime handle is invalid")
    }

    return Unmanaged<BackgroundRuntimeBridge>.fromOpaque(rawPointer).takeUnretainedValue()
}

@available(iOS 13.0, *)
private func mapSchedulerError(_ error: Error) -> BackgroundBridgeError {
    let nsError = error as NSError
    if nsError.domain == BGTaskScheduler.errorDomain {
        return .schedulerRejected(nsError.code, nsError.localizedDescription)
    }
    return .platform(nsError.localizedDescription)
}

private func wrapError(_ error: Error) -> RustString {
    if let bridgeError = error as? BackgroundBridgeError {
        return bridgeError.encoded()
    }
    return BackgroundBridgeError.platform(error.localizedDescription).encoded()
}

public func ios_background_initialize(event_ctx: UInt64, registrations_json: RustStr) -> RustString {
    if #available(iOS 13.0, *) {
        do {
            let registrationsString = registrations_json.toString()
            guard let registrationsData = registrationsString.data(using: .utf8) else {
                return RustString("err:config_missing:registrations_json is not UTF-8")
            }

            let registrations = try JSONDecoder().decode([RegistrationEntry].self, from: registrationsData)
            let runtime = BackgroundRuntimeBridge(eventCtx: event_ctx)
            try runtime.register(registrations)

            let handle = UInt64(UInt(bitPattern: Unmanaged.passRetained(runtime).toOpaque()))
            runtime.bindRuntimeHandle(handle)

            return RustString("ok:\(handle)")
        } catch {
            return RustString("err:\(wrapError(error).toString())")
        }
    }

    return RustString("err:not_supported")
}

public func ios_background_shutdown(runtime_handle: UInt64) {
    if #available(iOS 13.0, *) {
        guard runtime_handle != 0,
              let rawPointer = UnsafeRawPointer(bitPattern: UInt(runtime_handle))
        else {
            return
        }

        let runtime = Unmanaged<BackgroundRuntimeBridge>.fromOpaque(rawPointer).takeRetainedValue()
        runtime.shutdown()
    }
}

public func ios_background_capabilities() -> UInt8 {
    if #available(iOS 13.0, *) {
        var capabilities = capabilityAppRefresh | capabilityProcessing | capabilityLaunchEvents
#if WATERKIT_HAS_IOS26_BACKGROUND_TASKS
        if #available(iOS 26.0, *) {
            capabilities |= capabilityContinuedProcessing
            if BGTaskScheduler.supportedResources.contains(.gpu) {
                capabilities |= capabilityContinuedGPU
            }
        }
#endif
        return capabilities
    }

    return 0
}

public func ios_background_submit_app_refresh(
    runtime_handle: UInt64,
    identifier: RustStr,
    earliest_begin_seconds: UInt64
) -> Optional<RustString> {
    if #available(iOS 13.0, *) {
        do {
            let runtime = try runtimeFromHandle(runtime_handle)
            try runtime.submitAppRefresh(
                identifier: identifier.toString(),
                earliestBeginSeconds: earliest_begin_seconds
            )
            return nil
        } catch {
            return wrapError(error)
        }
    }

    return BackgroundBridgeError.notSupported.encoded()
}

public func ios_background_submit_processing(
    runtime_handle: UInt64,
    identifier: RustStr,
    earliest_begin_seconds: UInt64,
    requires_network_connectivity: Bool,
    requires_external_power: Bool
) -> Optional<RustString> {
    if #available(iOS 13.0, *) {
        do {
            let runtime = try runtimeFromHandle(runtime_handle)
            try runtime.submitProcessing(
                identifier: identifier.toString(),
                earliestBeginSeconds: earliest_begin_seconds,
                requiresNetworkConnectivity: requires_network_connectivity,
                requiresExternalPower: requires_external_power
            )
            return nil
        } catch {
            return wrapError(error)
        }
    }

    return BackgroundBridgeError.notSupported.encoded()
}

public func ios_background_submit_continued_processing(
    runtime_handle: UInt64,
    identifier: RustStr,
    title: RustStr,
    subtitle: RustStr,
    strategy: UInt8,
    requires_gpu: Bool
) -> Optional<RustString> {
    if #available(iOS 13.0, *) {
        do {
            let runtime = try runtimeFromHandle(runtime_handle)
            try runtime.submitContinuedProcessing(
                identifier: identifier.toString(),
                title: title.toString(),
                subtitle: subtitle.toString(),
                strategy: strategy,
                requiresGPU: requires_gpu
            )
            return nil
        } catch {
            return wrapError(error)
        }
    }

    return BackgroundBridgeError.notSupported.encoded()
}

public func ios_background_cancel(runtime_handle: UInt64, identifier: RustStr) -> Optional<RustString> {
    if #available(iOS 13.0, *) {
        do {
            let runtime = try runtimeFromHandle(runtime_handle)
            runtime.cancel(identifier: identifier.toString())
            return nil
        } catch {
            return wrapError(error)
        }
    }

    return BackgroundBridgeError.notSupported.encoded()
}

public func ios_background_cancel_all(runtime_handle: UInt64) -> Optional<RustString> {
    if #available(iOS 13.0, *) {
        do {
            let runtime = try runtimeFromHandle(runtime_handle)
            runtime.cancelAll()
            return nil
        } catch {
            return wrapError(error)
        }
    }

    return BackgroundBridgeError.notSupported.encoded()
}

public func ios_background_complete_task(
    runtime_handle: UInt64,
    task_token: UInt64,
    success: Bool
) -> Optional<RustString> {
    if #available(iOS 13.0, *) {
        do {
            let runtime = try runtimeFromHandle(runtime_handle)
            try runtime.completeTask(taskToken: task_token, success: success)
            return nil
        } catch {
            return wrapError(error)
        }
    }

    return BackgroundBridgeError.notSupported.encoded()
}

public func ios_background_update_continued_status(
    runtime_handle: UInt64,
    task_token: UInt64,
    title: RustStr,
    subtitle: RustStr
) -> Optional<RustString> {
    if #available(iOS 13.0, *) {
        do {
            let runtime = try runtimeFromHandle(runtime_handle)
            try runtime.updateContinuedStatus(
                taskToken: task_token,
                title: title.toString(),
                subtitle: subtitle.toString()
            )
            return nil
        } catch {
            return wrapError(error)
        }
    }

    return BackgroundBridgeError.notSupported.encoded()
}

public func ios_background_update_continued_progress(
    runtime_handle: UInt64,
    task_token: UInt64,
    completed: UInt64,
    total: UInt64
) -> Optional<RustString> {
    if #available(iOS 13.0, *) {
        do {
            let runtime = try runtimeFromHandle(runtime_handle)
            try runtime.updateContinuedProgress(
                taskToken: task_token,
                completed: completed,
                total: total
            )
            return nil
        } catch {
            return wrapError(error)
        }
    }

    return BackgroundBridgeError.notSupported.encoded()
}
