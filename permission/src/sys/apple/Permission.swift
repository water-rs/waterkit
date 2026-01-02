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
        return requestLocationPermission()
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

class LocationPermissionDelegate: NSObject, CLLocationManagerDelegate {
    var authorizationStatus: CLAuthorizationStatus?
    var completed = false
    var isFirstCallback = true

    func locationManagerDidChangeAuthorization(_ manager: CLLocationManager) {
        let status = manager.authorizationStatus
        // Skip the first callback which fires immediately with current status
        if isFirstCallback {
            isFirstCallback = false
            // If already determined on first callback, we're done
            if status != .notDetermined {
                authorizationStatus = status
                completed = true
            }
            return
        }
        // Subsequent callbacks indicate actual status change
        authorizationStatus = status
        completed = true
    }
}

private var locationManager: CLLocationManager?
private var locationDelegate: LocationPermissionDelegate?

private func requestLocationPermission() -> PermissionResult {
    let manager = CLLocationManager()
    let delegate = LocationPermissionDelegate()
    manager.delegate = delegate

    // Keep references alive
    locationManager = manager
    locationDelegate = delegate

    // Check if already determined
    let currentStatus = manager.authorizationStatus
    if currentStatus != .notDetermined {
        return statusFromCLAuthorizationStatus(currentStatus)
    }

    // Request authorization
    manager.requestWhenInUseAuthorization()

    // Run the main run loop to allow the authorization dialog to appear
    let timeout = Date().addingTimeInterval(60)
    while !delegate.completed && Date() < timeout {
        RunLoop.current.run(until: Date().addingTimeInterval(0.1))
    }

    if let status = delegate.authorizationStatus {
        return statusFromCLAuthorizationStatus(status)
    }

    return checkLocationPermission()
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
