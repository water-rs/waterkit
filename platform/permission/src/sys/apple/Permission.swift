import Foundation
import CoreLocation
import AVFoundation
import Photos
import Contacts
import EventKit

// Swift implementations of the functions declared in extern "Swift" block.
// swift-bridge generates the FFI glue - we just implement the functions.

func check_permission(permission: PermissionType) -> PermissionResult {
    switch permission {
    case .Location:
        return checkLocationPermission()
    case .Camera:
        return checkCameraPermission()
    case .Microphone:
        return checkMicrophonePermission()
    case .Photos:
        return checkPhotosPermission()
    case .Contacts:
        return checkContactsPermission()
    case .Calendar:
        return checkCalendarPermission()
    }
}

func request_permission(permission: PermissionType) -> PermissionResult {
    switch permission {
    case .Location:
        return checkLocationPermission()
    case .Camera:
        return requestCameraPermission()
    case .Microphone:
        return requestMicrophonePermission()
    case .Photos:
        return requestPhotosPermission()
    case .Contacts:
        return requestContactsPermission()
    case .Calendar:
        return requestCalendarPermission()
    }
}

// MARK: - Request Implementations

private final class LocationPermissionRequest: NSObject, CLLocationManagerDelegate {
    private let manager = CLLocationManager()
    private var callback: ((PermissionResult) -> Void)?
    private var keepAlive: LocationPermissionRequest?

    init(callback: @escaping (PermissionResult) -> Void) {
        self.callback = callback
    }

    func start() {
        keepAlive = self
        manager.delegate = self
        manager.requestWhenInUseAuthorization()
    }

    func locationManagerDidChangeAuthorization(_ manager: CLLocationManager) {
        let status = manager.authorizationStatus
        guard status != .notDetermined else {
            return
        }
        finish(statusFromCLAuthorizationStatus(status))
    }

    private func finish(_ result: PermissionResult) {
        guard let callback else {
            return
        }
        self.callback = nil
        manager.delegate = nil
        callback(result)
        keepAlive = nil
    }
}

func request_location_permission(callback: @escaping (PermissionResult) -> Void) {
    DispatchQueue.main.async {
        LocationPermissionRequest(callback: callback).start()
    }
}

private func statusFromCLAuthorizationStatus(_ status: CLAuthorizationStatus) -> PermissionResult {
    switch status {
    case .notDetermined:
        return .NotDetermined
    case .restricted:
        return .Restricted
    case .denied:
        return .Denied
    case .authorizedAlways, .authorizedWhenInUse:
        return .Granted
    @unknown default:
        return .NotDetermined
    }
}

private func requestCameraPermission() -> PermissionResult {
    let semaphore = DispatchSemaphore(value: 0)
    var result: PermissionResult = .NotDetermined
    AVCaptureDevice.requestAccess(for: .video) { granted in
        result = granted ? .Granted : .Denied
        semaphore.signal()
    }
    semaphore.wait()
    return result
}

private func requestMicrophonePermission() -> PermissionResult {
    let semaphore = DispatchSemaphore(value: 0)
    var result: PermissionResult = .NotDetermined
    AVCaptureDevice.requestAccess(for: .audio) { granted in
        result = granted ? .Granted : .Denied
        semaphore.signal()
    }
    semaphore.wait()
    return result
}

private func requestPhotosPermission() -> PermissionResult {
    let semaphore = DispatchSemaphore(value: 0)
    var result: PermissionResult = .NotDetermined
    PHPhotoLibrary.requestAuthorization { status in
        switch status {
        case .authorized, .limited:
            result = .Granted
        case .denied:
            result = .Denied
        case .restricted:
            result = .Restricted
        case .notDetermined:
            result = .NotDetermined
        @unknown default:
            result = .NotDetermined
        }
        semaphore.signal()
    }
    semaphore.wait()
    return result
}

private func requestContactsPermission() -> PermissionResult {
    let semaphore = DispatchSemaphore(value: 0)
    var result: PermissionResult = .NotDetermined
    let store = CNContactStore()
    store.requestAccess(for: .contacts) { granted, _ in
        result = granted ? .Granted : .Denied
        semaphore.signal()
    }
    semaphore.wait()
    return result
}

private func requestCalendarPermission() -> PermissionResult {
    let semaphore = DispatchSemaphore(value: 0)
    var result: PermissionResult = .NotDetermined
    let store = EKEventStore()
    if #available(macOS 14.0, iOS 17.0, *) {
        store.requestFullAccessToEvents { granted, _ in
            result = granted ? .Granted : .Denied
            semaphore.signal()
        }
    } else {
        store.requestAccess(to: .event) { granted, _ in
            result = granted ? .Granted : .Denied
            semaphore.signal()
        }
    }
    semaphore.wait()
    return result
}

// MARK: - Location

private func checkLocationPermission() -> PermissionResult {
    let status = CLLocationManager.authorizationStatus()
    switch status {
    case .notDetermined:
        return .NotDetermined
    case .restricted:
        return .Restricted
    case .denied:
        return .Denied
    case .authorizedAlways, .authorizedWhenInUse:
        return .Granted
    @unknown default:
        return .NotDetermined
    }
}

// MARK: - Camera

private func checkCameraPermission() -> PermissionResult {
    let status = AVCaptureDevice.authorizationStatus(for: .video)
    switch status {
    case .notDetermined:
        return .NotDetermined
    case .restricted:
        return .Restricted
    case .denied:
        return .Denied
    case .authorized:
        return .Granted
    @unknown default:
        return .NotDetermined
    }
}

// MARK: - Microphone

private func checkMicrophonePermission() -> PermissionResult {
    let status = AVCaptureDevice.authorizationStatus(for: .audio)
    switch status {
    case .notDetermined:
        return .NotDetermined
    case .restricted:
        return .Restricted
    case .denied:
        return .Denied
    case .authorized:
        return .Granted
    @unknown default:
        return .NotDetermined
    }
}

// MARK: - Photos

private func checkPhotosPermission() -> PermissionResult {
    let status = PHPhotoLibrary.authorizationStatus()
    switch status {
    case .notDetermined:
        return .NotDetermined
    case .restricted:
        return .Restricted
    case .denied:
        return .Denied
    case .authorized, .limited:
        return .Granted
    @unknown default:
        return .NotDetermined
    }
}

// MARK: - Contacts

private func checkContactsPermission() -> PermissionResult {
    let status = CNContactStore.authorizationStatus(for: .contacts)
    switch status {
    case .notDetermined:
        return .NotDetermined
    case .restricted:
        return .Restricted
    case .denied:
        return .Denied
    case .authorized:
        return .Granted
    @unknown default:
        return .NotDetermined
    }
}

// MARK: - Calendar

private func checkCalendarPermission() -> PermissionResult {
    let status = EKEventStore.authorizationStatus(for: .event)
    switch status {
    case .notDetermined:
        return .NotDetermined
    case .restricted:
        return .Restricted
    case .denied:
        return .Denied
    case .fullAccess, .writeOnly:
        return .Granted
    @unknown default:
        return .NotDetermined
    }
}
