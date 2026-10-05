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

/// Starts the system request flow for `permission` and reports the outcome
/// through `callback` once the user answers (or immediately, when the status
/// is already decided). Never blocks the calling thread: the Rust side awaits
/// the callback, so a caller can bound the wait on a prompt nobody answers.
func request_permission(permission: PermissionType, callback: @escaping (PermissionResult) -> Void) {
    switch permission {
    case .Location:
        DispatchQueue.main.async {
            LocationPermissionRequest(callback: callback).start()
        }
    case .Camera:
        AVCaptureDevice.requestAccess(for: .video) { granted in
            callback(granted ? .Granted : .Denied)
        }
    case .Microphone:
        AVCaptureDevice.requestAccess(for: .audio) { granted in
            callback(granted ? .Granted : .Denied)
        }
    case .Photos:
        PHPhotoLibrary.requestAuthorization { _ in
            callback(checkPhotosPermission())
        }
    case .Contacts:
        CNContactStore().requestAccess(for: .contacts) { granted, _ in
            callback(granted ? .Granted : .Denied)
        }
    case .Calendar:
        let store = EKEventStore()
        if #available(macOS 14.0, iOS 17.0, *) {
            store.requestFullAccessToEvents { granted, _ in
                callback(granted ? .Granted : .Denied)
            }
        } else {
            store.requestAccess(to: .event) { granted, _ in
                callback(granted ? .Granted : .Denied)
            }
        }
    }
}

// MARK: - Location request

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
