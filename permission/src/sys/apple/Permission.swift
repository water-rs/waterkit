import Foundation
import CoreLocation
import AVFoundation
import Photos
import Contacts
import EventKit

// Swift implementations of the functions declared in extern "Swift" block.
// swift-bridge generates the FFI glue - we just implement the functions.

func check_permission(_ permission: PermissionType) -> PermissionResult {
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

func request_permission(_ permission: PermissionType) -> PermissionResult {
    // For now, we just check status. Full async permission request
    // would require callback-based API.
    return check_permission(permission)
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
